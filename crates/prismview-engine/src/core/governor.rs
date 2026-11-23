//! `PowerGovernor` (Fase 5): máquina de estados de energia do motor. Independente de SO.
//!
//! Estados: `Normal`, `Eco` (30 FPS, sem pós-processamento), `Ultra` (15 FPS), `Paused` (jogo em
//! tela cheia, janela cobrindo o desktop, monitor desligado/sessão bloqueada, pausa do Studio) e
//! `Frozen` (1 quadro estático, depois o loop dorme).
//!
//! O governador não toca em GPU nem em Win32: recebe `GovernorInputs` (a plataforma/`native.rs`
//! preenche) e devolve decisões — `PowerCaps` (teto de FPS, pós-processamento, escala de render),
//! `FrameGate` (renderizar agora, esperar até X ou bloquear) e `HibernateAction` (soltar/retomar
//! render targets pesados). Quem tem os recursos (`SceneRenderer`, `Scene3dRenderer`) executa.
//!
//! Histerese:
//! - Modo AUTO usa a bateria como o Studio (`powerGovernor.ts`): Ultra entra em ≤20 % e só sai
//!   acima de 25 %; Eco vale enquanto estiver na bateria/economia de energia.
//! - Piorar (Normal → Eco → Ultra) é imediato; melhorar só depois de `UPGRADE_SETTLE` estável,
//!   para não oscilar ao plugar/desplugar o carregador.
//! - `Paused` é imediato nas duas pontas (retomar precisa ser instantâneo; não custa nada
//!   porque nada é liberado antes de `HIBERNATE_AFTER`).
//!
//! Hibernação de VRAM: após `HIBERNATE_AFTER` contínuos em `Paused`, emite `Release` uma vez
//! (o dono solta pirâmide de blur e alvos do 3D, mantendo cena/modelos/estado). Ao sair de
//! `Paused` com VRAM solta emite `Restore`; os alvos são recriados sob demanda no próximo quadro.
//!
//! Regras do plano: zero `unwrap/expect`, zero alocação (só `Copy`), nenhum timer: o chamador
//! inclui `next_deadline()` no seu `WaitUntil`.

use std::time::{Duration, Instant};

/// Teto de FPS no modo Eco.
pub const ECO_FPS: u32 = 30;
/// Teto de FPS no modo Ultra.
pub const ULTRA_FPS: u32 = 15;
/// Tempo contínuo em `Paused` antes de liberar a VRAM pesada.
pub const HIBERNATE_AFTER: Duration = Duration::from_secs(60);
/// Estabilidade exigida antes de voltar a um modo mais pesado.
pub const UPGRADE_SETTLE: Duration = Duration::from_secs(2);
/// Bateria (%) em que o AUTO entra em Ultra.
pub const ULTRA_ENTER_PERCENT: f32 = 20.0;
/// Bateria (%) acima da qual o AUTO sai do Ultra.
pub const ULTRA_EXIT_PERCENT: f32 = 25.0;

// ───────────────────────── Tipos públicos ─────────────────────────

/// Escolha do usuário (mesmos nomes do Studio: AUTO/NORMAL/ECO/ULTRA/FROZEN).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerPreference {
    #[default]
    Auto,
    Normal,
    Eco,
    Ultra,
    Frozen,
}

impl PowerPreference {
    /// Aceita `AUTO|NORMAL|ECO|ULTRA|FROZEN` (sem diferenciar caixa).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let is = |name: &str| s.eq_ignore_ascii_case(name);
        if is("auto") {
            Some(Self::Auto)
        } else if is("normal") {
            Some(Self::Normal)
        } else if is("eco") {
            Some(Self::Eco)
        } else if is("ultra") {
            Some(Self::Ultra)
        } else if is("frozen") || is("freeze") {
            Some(Self::Frozen)
        } else {
            None
        }
    }
}

/// Estado efetivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Normal,
    Eco,
    Ultra,
    Paused,
    Frozen,
}

impl PowerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Eco => "eco",
            Self::Ultra => "ultra",
            Self::Paused => "paused",
            Self::Frozen => "frozen",
        }
    }
}

/// Por que o motor está pausado (prioridade: usuário > monitor desligado > coberto).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// Pedido explícito (IPC `SetPaused`).
    User,
    /// Todos os monitores desligados / sessão bloqueada.
    DisplayOff,
    /// Nenhum monitor com desktop visível (jogo em tela cheia, janela maximizada).
    Covered,
}

impl PauseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "ipc",
            Self::DisplayOff => "display_off",
            Self::Covered => "covered",
        }
    }
}

/// Fatos do mundo, preenchidos pela plataforma a cada iteração do loop.
#[derive(Debug, Clone, Copy, Default)]
pub struct GovernorInputs {
    pub preference: PowerPreference,
    /// Pausa pedida pelo Studio/IPC.
    pub user_paused: bool,
    /// Algum monitor com o desktop visível.
    pub any_visible: bool,
    /// Há monitores e todos estão desligados/bloqueados.
    pub display_off: bool,
    pub on_battery: bool,
    /// `None` = sem bateria/desconhecida.
    pub battery_percent: Option<f32>,
    pub battery_saver: bool,
}

/// Orçamento do estado atual; os renderizadores só obedecem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerCaps {
    /// `None` = sem teto (cadência do sistema/eventos).
    pub fps_cap: Option<u32>,
    /// Blur do vidro e demais passes opcionais.
    pub post_processing: bool,
    /// Teto da escala de render do 3D (`Scene3dRenderer::set_render_scale_cap`).
    pub render_scale_cap: f32,
    /// Teto de MSAA do 3D (4 ou 1; `Scene3dRenderer::set_msaa_cap`).
    pub msaa: u32,
}

impl PowerCaps {
    /// Menor intervalo entre quadros (`ZERO` = sem teto).
    pub fn min_frame_interval(&self) -> Duration {
        match self.fps_cap {
            Some(fps) if fps > 0 => Duration::from_nanos(1_000_000_000 / u64::from(fps)),
            _ => Duration::ZERO,
        }
    }
}

/// Ação sobre a VRAM pesada, emitida uma vez por transição.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HibernateAction {
    None,
    /// Soltar render targets pesados (blur, HDR/MSAA do 3D…).
    Release,
    /// Saiu de `Paused` com a VRAM solta: forçar redesenho (recriação é sob demanda).
    Restore,
}

/// Resultado de `update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GovernorUpdate {
    pub state_changed: bool,
    pub caps_changed: bool,
    pub hibernate: HibernateAction,
}

impl GovernorUpdate {
    /// O estado mudou de forma que exige redesenhar (caps, retomada, congelamento).
    pub fn needs_redraw(&self) -> bool {
        self.state_changed || self.caps_changed || self.hibernate == HibernateAction::Restore
    }
}

/// Decisão de quadro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameGate {
    /// Pode renderizar agora.
    Now,
    /// Teto de FPS: tentar de novo neste instante.
    At(Instant),
    /// Não renderizar (pausado ou congelado com o quadro já entregue).
    Blocked,
}

// ───────────────────────── Governador ─────────────────────────

/// Nível "base" derivado da preferência/bateria (sem pausa/congelamento).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Normal,
    Eco,
    Ultra,
}

impl Level {
    fn rank(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Eco => 1,
            Self::Ultra => 2,
        }
    }

    fn caps(self) -> PowerCaps {
        match self {
            Self::Normal => PowerCaps {
                fps_cap: None,
                post_processing: true,
                render_scale_cap: 1.0,
                msaa: 4,
            },
            Self::Eco => PowerCaps {
                fps_cap: Some(ECO_FPS),
                post_processing: false,
                render_scale_cap: 0.85,
                msaa: 1,
            },
            Self::Ultra => PowerCaps {
                fps_cap: Some(ULTRA_FPS),
                post_processing: false,
                render_scale_cap: 0.6,
                msaa: 1,
            },
        }
    }
}

pub struct PowerGovernor {
    state: PowerState,
    caps: PowerCaps,
    level: Level,
    /// Nível mais leve candidato, com o instante em que apareceu (histerese de subida).
    upgrade: Option<(Level, Instant)>,
    pause_reason: Option<PauseReason>,
    paused_since: Option<Instant>,
    hibernated: bool,
    /// Em `Frozen`: o quadro estático ainda não foi entregue.
    frozen_pending: bool,
    last_frame: Option<Instant>,
}

impl Default for PowerGovernor {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerGovernor {
    pub fn new() -> Self {
        Self {
            state: PowerState::Normal,
            caps: Level::Normal.caps(),
            level: Level::Normal,
            upgrade: None,
            pause_reason: None,
            paused_since: None,
            hibernated: false,
            frozen_pending: false,
            last_frame: None,
        }
    }

    pub fn state(&self) -> PowerState {
        self.state
    }

    pub fn caps(&self) -> PowerCaps {
        self.caps
    }

    pub fn pause_reason(&self) -> Option<PauseReason> {
        self.pause_reason
    }

    /// Render targets pesados estão liberados (`Release` já emitido e ainda não retomado).
    pub fn is_hibernated(&self) -> bool {
        self.hibernated
    }

    /// Reavalia o estado. Barato e sem alocação: chamar a cada iteração do loop.
    pub fn update(&mut self, now: Instant, inputs: &GovernorInputs) -> GovernorUpdate {
        self.update_level(now, inputs);

        let pause = if inputs.user_paused {
            Some(PauseReason::User)
        } else if inputs.display_off {
            Some(PauseReason::DisplayOff)
        } else if !inputs.any_visible {
            Some(PauseReason::Covered)
        } else {
            None
        };

        let new_state = if pause.is_some() {
            PowerState::Paused
        } else if inputs.preference == PowerPreference::Frozen {
            PowerState::Frozen
        } else {
            match self.level {
                Level::Normal => PowerState::Normal,
                Level::Eco => PowerState::Eco,
                Level::Ultra => PowerState::Ultra,
            }
        };

        // Caps: Paused/Frozen mantêm a qualidade do nível (retomar sem salto visual).
        let new_caps = match new_state {
            PowerState::Frozen => Level::Normal.caps(),
            _ => self.level.caps(),
        };

        let mut hibernate = HibernateAction::None;
        let was_paused = self.state == PowerState::Paused;
        if new_state == PowerState::Paused {
            if self.paused_since.is_none() {
                self.paused_since = Some(now);
            }
            if !self.hibernated
                && self
                    .paused_since
                    .is_some_and(|since| now.saturating_duration_since(since) >= HIBERNATE_AFTER)
            {
                self.hibernated = true;
                hibernate = HibernateAction::Release;
            }
        } else {
            self.paused_since = None;
            if self.hibernated {
                self.hibernated = false;
                hibernate = HibernateAction::Restore;
            }
        }
        self.pause_reason = pause;

        let state_changed = new_state != self.state;
        let caps_changed = new_caps != self.caps;
        if state_changed {
            // Entrar em Frozen exige exatamente 1 quadro novo; sair de Paused também redesenha.
            self.frozen_pending = new_state == PowerState::Frozen;
            if was_paused {
                self.last_frame = None; // retomada instantânea: sem esperar o teto de FPS
            }
        }
        self.state = new_state;
        self.caps = new_caps;

        GovernorUpdate {
            state_changed,
            caps_changed,
            hibernate,
        }
    }

    /// Calcula o nível base com histerese (preferência + bateria).
    fn update_level(&mut self, now: Instant, inputs: &GovernorInputs) {
        let target = match inputs.preference {
            PowerPreference::Normal | PowerPreference::Frozen => Level::Normal,
            PowerPreference::Eco => Level::Eco,
            PowerPreference::Ultra => Level::Ultra,
            PowerPreference::Auto => self.auto_level(inputs),
        };

        if target.rank() >= self.level.rank() {
            // Igual ou mais pesado: imediato.
            self.level = target;
            self.upgrade = None;
            return;
        }
        // Mais leve: exige estabilidade.
        match self.upgrade {
            Some((candidate, since)) if candidate == target => {
                if now.saturating_duration_since(since) >= UPGRADE_SETTLE {
                    self.level = target;
                    self.upgrade = None;
                }
            }
            _ => self.upgrade = Some((target, now)),
        }
    }

    /// AUTO: tomada = Normal; bateria/economia = Eco; ≤20 % = Ultra (sai acima de 25 %).
    fn auto_level(&self, inputs: &GovernorInputs) -> Level {
        if !inputs.on_battery && !inputs.battery_saver {
            return Level::Normal;
        }
        let percent = inputs.battery_percent.filter(|p| p.is_finite());
        let ultra = match percent {
            Some(p) if self.level == Level::Ultra => p <= ULTRA_EXIT_PERCENT,
            Some(p) => p <= ULTRA_ENTER_PERCENT,
            None => false,
        };
        if ultra {
            Level::Ultra
        } else {
            Level::Eco
        }
    }

    /// Prazo interno que precisa acordar o loop (hibernação, subida de nível). `None` = nenhum.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let hibernate = if self.state == PowerState::Paused && !self.hibernated {
            self.paused_since
                .and_then(|since| since.checked_add(HIBERNATE_AFTER))
        } else {
            None
        };
        let upgrade = self
            .upgrade
            .and_then(|(_, since)| since.checked_add(UPGRADE_SETTLE));
        [hibernate, upgrade]
            .into_iter()
            .flatten()
            .map(|at| at.max(now))
            .min()
    }

    /// Pode renderizar agora? Aplica pausa, congelamento e o teto de FPS.
    pub fn frame_gate(&self, now: Instant) -> FrameGate {
        match self.state {
            PowerState::Paused => return FrameGate::Blocked,
            PowerState::Frozen if !self.frozen_pending => return FrameGate::Blocked,
            _ => {}
        }
        let interval = self.caps.min_frame_interval();
        if interval.is_zero() {
            return FrameGate::Now;
        }
        match self.last_frame.and_then(|last| last.checked_add(interval)) {
            Some(due) if now < due => FrameGate::At(due),
            _ => FrameGate::Now,
        }
    }

    /// Chamar após apresentar um quadro (alimenta o teto de FPS e conclui o quadro congelado).
    pub fn frame_presented(&mut self, now: Instant) {
        self.last_frame = Some(now);
        self.frozen_pending = false;
    }

    /// O conteúdo mudou (cena nova, monitores refeitos, resize): em `Frozen` libera 1 quadro novo.
    pub fn invalidate_frozen(&mut self) {
        if self.state == PowerState::Frozen {
            self.frozen_pending = true;
        }
    }

    /// Cadência para animações do loop (molas, vídeo): intervalo mínimo entre quadros.
    pub fn frame_interval(&self) -> Duration {
        self.caps.min_frame_interval()
    }

    /// Animações contínuas (relógio, molas, vídeo) devem rodar? `false` em `Paused`/`Frozen`.
    pub fn animations_allowed(&self) -> bool {
        !matches!(self.state, PowerState::Paused | PowerState::Frozen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible() -> GovernorInputs {
        GovernorInputs {
            any_visible: true,
            ..GovernorInputs::default()
        }
    }

    #[test]
    fn auto_battery_hysteresis() {
        let t0 = Instant::now();
        let mut g = PowerGovernor::new();
        let mut i = visible();
        i.on_battery = true;
        i.battery_percent = Some(60.0);
        g.update(t0, &i);
        assert_eq!(g.state(), PowerState::Eco);
        i.battery_percent = Some(20.0);
        g.update(t0, &i);
        assert_eq!(g.state(), PowerState::Ultra);
        // 23 %: continua Ultra (só sai acima de 25 %).
        i.battery_percent = Some(23.0);
        g.update(t0 + Duration::from_secs(10), &i);
        assert_eq!(g.state(), PowerState::Ultra);
        // 30 %: precisa estabilizar antes de subir.
        i.battery_percent = Some(30.0);
        let t1 = t0 + Duration::from_secs(20);
        g.update(t1, &i);
        assert_eq!(g.state(), PowerState::Ultra);
        g.update(t1 + UPGRADE_SETTLE, &i);
        assert_eq!(g.state(), PowerState::Eco);
    }

    #[test]
    fn hibernates_after_sixty_seconds_and_restores() {
        let t0 = Instant::now();
        let mut g = PowerGovernor::new();
        g.update(t0, &visible());
        let covered = GovernorInputs::default();
        let u = g.update(t0, &covered);
        assert_eq!(g.state(), PowerState::Paused);
        assert_eq!(u.hibernate, HibernateAction::None);
        assert_eq!(g.frame_gate(t0), FrameGate::Blocked);
        let u = g.update(t0 + HIBERNATE_AFTER, &covered);
        assert_eq!(u.hibernate, HibernateAction::Release);
        assert!(g.is_hibernated());
        // Só emite uma vez.
        let u = g.update(t0 + HIBERNATE_AFTER + Duration::from_secs(5), &covered);
        assert_eq!(u.hibernate, HibernateAction::None);
        let u = g.update(t0 + Duration::from_secs(70), &visible());
        assert_eq!(u.hibernate, HibernateAction::Restore);
        assert_eq!(g.state(), PowerState::Normal);
        assert_eq!(g.frame_gate(t0 + Duration::from_secs(70)), FrameGate::Now);
    }

    #[test]
    fn frozen_renders_exactly_one_frame() {
        let t0 = Instant::now();
        let mut g = PowerGovernor::new();
        let mut i = visible();
        i.preference = PowerPreference::Frozen;
        g.update(t0, &i);
        assert_eq!(g.state(), PowerState::Frozen);
        assert_eq!(g.frame_gate(t0), FrameGate::Now);
        g.frame_presented(t0);
        assert_eq!(g.frame_gate(t0), FrameGate::Blocked);
        g.invalidate_frozen();
        assert_eq!(g.frame_gate(t0), FrameGate::Now);
    }

    #[test]
    fn eco_caps_frame_rate() {
        let t0 = Instant::now();
        let mut g = PowerGovernor::new();
        let mut i = visible();
        i.preference = PowerPreference::Eco;
        g.update(t0, &i);
        assert!(!g.caps().post_processing);
        g.frame_presented(t0);
        assert!(matches!(g.frame_gate(t0 + Duration::from_millis(5)), FrameGate::At(_)));
        assert_eq!(g.frame_gate(t0 + Duration::from_millis(40)), FrameGate::Now);
    }
}
