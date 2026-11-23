//! SMTC / GSMTC (Global System Media Transport Controls): música atual, posição, capa do álbum em
//! disco e cor dominante da capa. Porte do `spawn_gsmtc_worker` do Estúdio
//! (`src-tauri/src/desktop_host.rs`), dirigido por eventos WinRT:
//! `CurrentSessionChanged`, `PlaybackInfoChanged`, `MediaPropertiesChanged`,
//! `TimelinePropertiesChanged`. Sem polling: a thread dorme num `Condvar` sem timeout quando nada
//! toca; tocando, só ressincroniza a linha do tempo a cada `TIMELINE_RESYNC` (o consumidor
//! extrapola a posição entre eventos).
//!
//! Fora da demanda `MEDIA` os handlers da sessão são removidos; comandos de transporte
//! (play/pause/next/prev) continuam funcionando.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::windows::core::Interface;
use ::windows::Foundation::{
    AsyncOperationCompletedHandler, EventRegistrationToken, IAsyncOperation, TypedEventHandler,
};
use ::windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use ::windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};

use super::{ComMta, Shared};
use crate::engine_core::signals::{dominant_color, MediaScalars, MediaText, SignalKind};
use crate::platform::MediaCommand;

const ASYNC_TIMEOUT: Duration = Duration::from_millis(1500);
const COMMAND_TIMEOUT: Duration = Duration::from_millis(1200);
const MANAGER_RETRY: Duration = Duration::from_secs(5);
const TIMELINE_RESYNC: Duration = Duration::from_secs(5);
const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
const COVER_PREFIX: &str = "native_cover_";
const COVER_SAMPLE_PX: u32 = 32;
const MAX_PENDING_COMMANDS: usize = 16;
/// 1601-01-01 → 1970-01-01 em unidades de 100 ns (`DateTime::UniversalTime`).
const FILETIME_UNIX_OFFSET: i64 = 116_444_736_000_000_000;

#[derive(Default)]
struct Pending {
    dirty: bool,
    session_changed: bool,
    demand_changed: bool,
    stop: bool,
    commands: VecDeque<MediaCommand>,
}

impl Pending {
    fn any(&self) -> bool {
        self.dirty || self.session_changed || self.demand_changed || self.stop || !self.commands.is_empty()
    }
}

/// Caixa de correio do worker (eventos WinRT, demanda, comandos, parada).
#[derive(Default)]
pub(super) struct MediaChannel {
    state: Mutex<Pending>,
    cvar: Condvar,
}

impl MediaChannel {
    fn update(&self, f: impl FnOnce(&mut Pending)) {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        f(&mut guard);
        self.cvar.notify_one();
    }

    pub(super) fn notify_demand(&self) {
        self.update(|p| p.demand_changed = true);
    }

    pub(super) fn stop(&self) {
        self.update(|p| p.stop = true);
    }

    pub(super) fn push_command(&self, cmd: MediaCommand) {
        self.update(|p| {
            if p.commands.len() < MAX_PENDING_COMMANDS {
                p.commands.push_back(cmd);
            }
        });
    }

    fn mark_dirty(&self) {
        self.update(|p| p.dirty = true);
    }

    fn mark_session_changed(&self) {
        self.update(|p| {
            p.session_changed = true;
            p.dirty = true;
        });
    }

    /// Espera trabalho. `timeout = None` → dorme até um evento. Retorna o lote e se expirou.
    fn wait(&self, timeout: Option<Duration>, out: &mut Pending) -> bool {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let mut timed_out = false;
        if !guard.any() {
            match timeout {
                None => {
                    while !guard.any() {
                        guard = match self.cvar.wait(guard) {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                    }
                }
                Some(t) => {
                    let deadline = Instant::now() + t;
                    while !guard.any() {
                        let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                            timed_out = true;
                            break;
                        };
                        guard = match self.cvar.wait_timeout(guard, rem) {
                            Ok((g, _)) => g,
                            Err(p) => p.into_inner().0,
                        };
                    }
                }
            }
        }
        out.dirty = std::mem::take(&mut guard.dirty);
        out.session_changed = std::mem::take(&mut guard.session_changed);
        out.demand_changed = std::mem::take(&mut guard.demand_changed);
        out.stop = guard.stop;
        out.commands.clear();
        out.commands.extend(guard.commands.drain(..));
        timed_out
    }
}

/// Aguarda uma operação WinRT com prazo (o `get()` do windows-rs não tem timeout).
fn await_async<T>(op: IAsyncOperation<T>, timeout: Duration) -> Option<T>
where
    T: ::windows::core::RuntimeType + 'static,
{
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let done_cb = Arc::clone(&done);
    let handler = AsyncOperationCompletedHandler::new(move |_op, _status| {
        let (lock, cvar) = &*done_cb;
        if let Ok(mut flag) = lock.lock() {
            *flag = true;
            cvar.notify_one();
        }
        Ok(())
    });
    op.SetCompleted(&handler).ok()?;
    let (lock, cvar) = &*done;
    let mut guard = lock.lock().ok()?;
    let deadline = Instant::now() + timeout;
    while !*guard {
        let rem = deadline.checked_duration_since(Instant::now())?;
        guard = match cvar.wait_timeout(guard, rem) {
            Ok((g, res)) if res.timed_out() && !*g => return None,
            Ok((g, _)) => g,
            Err(p) => p.into_inner().0,
        };
    }
    drop(guard);
    op.GetResults().ok()
}

#[derive(Default)]
struct Tokens {
    playback: Option<EventRegistrationToken>,
    props: Option<EventRegistrationToken>,
    timeline: Option<EventRegistrationToken>,
}

struct Worker {
    shared: Arc<Shared>,
    channel: Arc<MediaChannel>,
    cover_dir: PathBuf,
    session: Option<Session>,
    session_ptr: usize,
    tokens: Tokens,
    text: MediaText,
    track_key: String,
    key_scratch: String,
    cover_file: Option<PathBuf>,
    cover_counter: u64,
    dominant: u32,
    playing: bool,
}

pub(super) fn run(shared: Arc<Shared>, channel: Arc<MediaChannel>, cover_dir: PathBuf) {
    let mut pending = Pending::default();
    let mut queued_commands: VecDeque<MediaCommand> = VecDeque::new();
    // Adia inicialização COM/WinRT até alguma cena pedir `MEDIA` ou chegar um comando de mídia.
    while !shared.stop.load(Ordering::Relaxed)
        && !shared.bus.is_demanded(SignalKind::MEDIA)
        && queued_commands.is_empty()
    {
        channel.wait(None, &mut pending);
        queued_commands.extend(pending.commands.drain(..));
        if pending.stop {
            return;
        }
    }
    if shared.stop.load(Ordering::Relaxed) {
        return;
    }

    let _com = ComMta::init();
    cleanup_stale_covers(&cover_dir);

    // Gerenciador: pode falhar logo após o logon; tenta de novo sem girar.
    let manager = loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(m) = SessionManager::RequestAsync()
            .ok()
            .and_then(|op| await_async(op, Duration::from_secs(3)))
        {
            break m;
        }
        channel.wait(Some(MANAGER_RETRY), &mut pending);
        queued_commands.extend(pending.commands.drain(..));
        if pending.stop {
            return;
        }
    };

    let ch = Arc::clone(&channel);
    let current_token = manager
        .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            ch.mark_session_changed();
            Ok(())
        }))
        .ok();

    let mut w = Worker {
        shared,
        channel,
        cover_dir,
        session: None,
        session_ptr: 0,
        tokens: Tokens::default(),
        text: MediaText::default(),
        track_key: String::new(),
        key_scratch: String::new(),
        cover_file: None,
        cover_counter: 0,
        dominant: 0,
        playing: false,
    };
    // Primeira leitura se a cena já pede mídia (preservando comandos enfileirados antes do init).
    pending.session_changed = true;
    pending.dirty = true;
    pending.commands.extend(queued_commands.drain(..));
    let mut first = true;

    loop {
        let mut timeline_due = false;
        if !first {
            let active = w.shared.bus.is_demanded(SignalKind::MEDIA);
            let timeout = (active && w.playing).then_some(TIMELINE_RESYNC);
            timeline_due = w.channel.wait(timeout, &mut pending);
        }
        first = false;
        if pending.stop || w.shared.stop.load(Ordering::Relaxed) {
            break;
        }

        let active = w.shared.bus.is_demanded(SignalKind::MEDIA);
        if !active {
            if w.session.is_some() {
                w.detach();
                w.forget_track();
                w.publish_empty();
            }
            if !pending.commands.is_empty() {
                if let Ok(s) = manager.GetCurrentSession() {
                    run_commands(&s, &pending.commands);
                }
            }
            continue;
        }

        let rebind = pending.session_changed || pending.demand_changed || w.session.is_none();
        if rebind {
            w.rebind(&manager);
        }
        if let Some(s) = &w.session {
            if !pending.commands.is_empty() {
                run_commands(s, &pending.commands);
            }
        }
        if rebind || pending.dirty || timeline_due {
            w.refresh(!(rebind || pending.dirty));
        }
    }

    w.detach();
    if let Some(old) = w.cover_file.take() {
        let _ = fs::remove_file(old);
    }
    if let Some(tok) = current_token {
        let _ = manager.RemoveCurrentSessionChanged(tok);
    }
}

fn run_commands(session: &Session, commands: &VecDeque<MediaCommand>) {
    for cmd in commands {
        let op = match cmd {
            MediaCommand::PlayPause => session.TryTogglePlayPauseAsync(),
            MediaCommand::Next => session.TrySkipNextAsync(),
            MediaCommand::Previous => session.TrySkipPreviousAsync(),
        };
        if let Ok(op) = op {
            let _ = await_async(op, COMMAND_TIMEOUT);
        }
    }
}

impl Worker {
    fn detach(&mut self) {
        if let Some(s) = self.session.take() {
            if let Some(t) = self.tokens.playback.take() {
                let _ = s.RemovePlaybackInfoChanged(t);
            }
            if let Some(t) = self.tokens.props.take() {
                let _ = s.RemoveMediaPropertiesChanged(t);
            }
            if let Some(t) = self.tokens.timeline.take() {
                let _ = s.RemoveTimelinePropertiesChanged(t);
            }
        }
        self.tokens = Tokens::default();
        self.session_ptr = 0;
    }

    fn rebind(&mut self, manager: &SessionManager) {
        let Ok(next) = manager.GetCurrentSession() else {
            self.detach();
            return;
        };
        // Identidade COM: trocar de aba/player no mesmo app também troca o objeto.
        let ptr = next.as_raw() as usize;
        if self.session.is_some() && self.session_ptr == ptr {
            return;
        }
        self.detach();

        let ch = Arc::clone(&self.channel);
        self.tokens.playback = next
            .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                ch.mark_dirty();
                Ok(())
            }))
            .ok();
        let ch = Arc::clone(&self.channel);
        self.tokens.props = next
            .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                ch.mark_dirty();
                Ok(())
            }))
            .ok();
        let ch = Arc::clone(&self.channel);
        self.tokens.timeline = next
            .TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                ch.mark_dirty();
                Ok(())
            }))
            .ok();
        self.session_ptr = ptr;
        self.session = Some(next);
    }

    fn forget_track(&mut self) {
        self.track_key.clear();
        self.text = MediaText::default();
        self.dominant = 0;
        self.playing = false;
        if let Some(old) = self.cover_file.take() {
            let _ = fs::remove_file(old);
        }
    }

    fn publish_empty(&self) {
        self.shared
            .bus
            .publish_media(MediaScalars::default(), Some(&MediaText::default()), true);
    }

    /// Lê a sessão e publica. `timeline_only`: só ressincroniza posição/estado.
    fn refresh(&mut self, timeline_only: bool) {
        let Some(session) = self.session.clone() else {
            if !self.track_key.is_empty() || !self.text.title.is_empty() {
                self.forget_track();
            }
            self.publish_empty();
            return;
        };

        let playing = session
            .GetPlaybackInfo()
            .and_then(|pi| pi.PlaybackStatus())
            .map(|st| st == PlaybackStatus::Playing)
            .unwrap_or(self.playing);
        self.playing = playing;

        let (position_ms, duration_ms) = read_timeline(&session, playing);

        let mut text_changed = false;
        let mut cover_changed = false;
        if !timeline_only {
            if let Some(props) = session
                .TryGetMediaPropertiesAsync()
                .ok()
                .and_then(|op| await_async(op, ASYNC_TIMEOUT))
            {
                let title = props.Title().map(|s| s.to_string_lossy()).unwrap_or_default();
                let artist = props.Artist().map(|s| s.to_string_lossy()).unwrap_or_default();
                let album = props.AlbumTitle().map(|s| s.to_string_lossy()).unwrap_or_default();
                let app = session
                    .SourceAppUserModelId()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();

                self.key_scratch.clear();
                for part in [&title, &artist, &album] {
                    self.key_scratch.push_str(part);
                    self.key_scratch.push('\u{1f}');
                }
                let track_changed = self.key_scratch != self.track_key;
                if track_changed || self.text.title != title || self.text.source_app != app {
                    text_changed = true;
                }
                self.text.title = title;
                self.text.artist = artist;
                self.text.album = album;
                self.text.source_app = app;

                // Capa: troca de faixa ou capa ainda ausente (miniaturas chegam atrasadas).
                if track_changed || self.cover_file.is_none() {
                    std::mem::swap(&mut self.track_key, &mut self.key_scratch);
                    let fetched = props
                        .Thumbnail()
                        .ok()
                        .and_then(|t| self.save_cover(&t));
                    match fetched {
                        Some((path, color)) => {
                            if let Some(old) = self.cover_file.replace(path) {
                                let _ = fs::remove_file(old);
                            }
                            self.dominant = color;
                            cover_changed = true;
                        }
                        None if track_changed => {
                            if let Some(old) = self.cover_file.take() {
                                let _ = fs::remove_file(old);
                                cover_changed = true;
                            }
                            self.dominant = 0;
                        }
                        None => {}
                    }
                    self.text.cover_path = self
                        .cover_file
                        .as_deref()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    text_changed |= cover_changed;
                }
            }
        }

        let scalars = MediaScalars {
            has_session: true,
            playing,
            position_ms,
            duration_ms,
            dominant_rgba: self.dominant,
        };
        let text = text_changed.then_some(&self.text);
        self.shared.bus.publish_media(scalars, text, cover_changed);
    }

    /// Lê a miniatura, salva em disco (o renderer carrega por caminho) e calcula a cor dominante.
    fn save_cover(&mut self, thumb: &IRandomAccessStreamReference) -> Option<(PathBuf, u32)> {
        let stream = await_async(thumb.OpenReadAsync().ok()?, ASYNC_TIMEOUT)?;
        let size = stream.Size().ok()?;
        if size == 0 || size > MAX_COVER_BYTES {
            return None;
        }
        let total = size as usize;
        let input = stream.GetInputStreamAt(0).ok()?;
        let reader = DataReader::CreateDataReader(&input).ok()?;
        let mut bytes: Vec<u8> = Vec::with_capacity(total);
        while bytes.len() < total {
            let want = (total - bytes.len()).min(u32::MAX as usize) as u32;
            let op = reader.LoadAsync(want).ok()?.cast::<IAsyncOperation<u32>>().ok()?;
            let loaded = await_async(op, ASYNC_TIMEOUT)? as usize;
            if loaded == 0 {
                break;
            }
            let start = bytes.len();
            bytes.resize(start + loaded, 0);
            reader.ReadBytes(&mut bytes[start..]).ok()?;
        }
        if bytes.is_empty() {
            return None;
        }

        let color = image::load_from_memory(&bytes)
            .ok()
            .map(|img| {
                let small = img.thumbnail(COVER_SAMPLE_PX, COVER_SAMPLE_PX).to_rgba8();
                dominant_color(small.as_raw())
            })
            .unwrap_or(0);

        let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            "png"
        } else if bytes.starts_with(b"RIFF") {
            "webp"
        } else {
            "jpg"
        };
        fs::create_dir_all(&self.cover_dir).ok()?;
        self.cover_counter = self.cover_counter.wrapping_add(1);
        let dest = self
            .cover_dir
            .join(format!("{COVER_PREFIX}{}_{}.{ext}", std::process::id(), self.cover_counter));
        fs::write(&dest, &bytes).ok()?;
        Some((dest, color))
    }
}

/// `(posição_ms, duração_ms)`; posição corrigida pelo `LastUpdatedTime` quando tocando.
fn read_timeline(session: &Session, playing: bool) -> (u64, u64) {
    let Ok(tl) = session.GetTimelineProperties() else {
        return (0, 0);
    };
    let ticks = |v: ::windows::core::Result<::windows::Foundation::TimeSpan>| {
        v.map(|t| t.Duration.max(0)).unwrap_or(0)
    };
    let start = ticks(tl.StartTime());
    let end = ticks(tl.EndTime());
    let mut pos = ticks(tl.Position()).saturating_sub(start);
    let duration = end.saturating_sub(start);

    if playing {
        if let Ok(updated) = tl.LastUpdatedTime() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| (d.as_nanos() / 100) as i64 + FILETIME_UNIX_OFFSET)
                .unwrap_or(0);
            let elapsed = now.saturating_sub(updated.UniversalTime);
            // Ignora carimbos absurdos (apps que nunca atualizam o campo).
            if updated.UniversalTime > 0 && (0..36_000_000_000).contains(&elapsed) {
                pos = pos.saturating_add(elapsed);
            }
        }
    }
    if duration > 0 {
        pos = pos.min(duration);
    }
    ((pos / 10_000) as u64, (duration / 10_000) as u64)
}

fn cleanup_stale_covers(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(COVER_PREFIX) {
            let _ = fs::remove_file(entry.path());
        }
    }
}
