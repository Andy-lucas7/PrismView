//! Hit-test de toques no desktop (Fase 4): camadas 2D com `touch.action_type` viram regiões
//! clicáveis (caixa orientada, mesma afim do desenho): mídia → SMTC; demais → `Custom`.
//!
//! Fluxo sem roubar o foco:
//! 1. a plataforma observa o mouse por Raw Input (`RIDEV_INPUTSINK`: só lê, não consome) e,
//!    num clique curto sobre o **fundo** do desktop (não em ícone, não em outra janela), publica
//!    `SignalBus::publish_desktop_tap` — nenhuma janela nossa é ativada, o Explorer recebe o
//!    clique normalmente;
//! 2. o loop do renderer acorda (bit `DESKTOP_TAP`), converte o ponto para a saída (monitor) e
//!    chama `HitMap::hit_test` com a lista construída no último rebuild da cena 2D;
//! 3. a ação vira `MediaCommand` → `SystemSensors::media_control` (fila da thread GSMTC).
//!
//! Sem SO aqui: só geometria. Regiões são reconstruídas junto com a lista de desenho (sem
//! alocação por quadro: o `Vec` mantém a capacidade).

use prism_core::schema::TouchActionType;

use crate::platform::MediaCommand;

/// Ação de uma região tocável no wallpaper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopAction {
    MediaPlayPause,
    MediaNext,
    MediaPrev,
    /// Demais `TouchAction` (abrir URL/app, alternar/definir global, próxima da lista...):
    /// índice na tabela de toques da cena (`SceneRenderer::touch_action`). O loop executa.
    Custom(u32),
}

impl DesktopAction {
    /// Ações de mídia (SMTC). As demais usam `Custom` com o índice da tabela da cena.
    pub fn from_touch(action: TouchActionType) -> Option<Self> {
        match action {
            TouchActionType::MediaPlayPause => Some(Self::MediaPlayPause),
            TouchActionType::MediaNext => Some(Self::MediaNext),
            TouchActionType::MediaPrev => Some(Self::MediaPrev),
            _ => None,
        }
    }

    /// Comando SMTC (`None` para ações `Custom`).
    pub fn media_command(self) -> Option<MediaCommand> {
        match self {
            Self::MediaPlayPause => Some(MediaCommand::PlayPause),
            Self::MediaNext => Some(MediaCommand::Next),
            Self::MediaPrev => Some(MediaCommand::Previous),
            Self::Custom(_) => None,
        }
    }
}

/// Caixa orientada em pixels físicos da saída: `p ∈ região ⇔ |M⁻¹(p - c)| ≤ half`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitRegion {
    center: [f32; 2],
    /// Inversa da parte linear (coluna-maior, como `Affine::m` do `scene2d`).
    inv: [f32; 4],
    half: [f32; 2],
    pub action: DesktopAction,
}

impl HitRegion {
    /// `m` = parte linear da afim da camada (rotação × escala acumuladas), `half` no espaço local.
    /// `None` se a transformação é degenerada.
    pub fn new(center: [f32; 2], m: [f32; 4], half: [f32; 2], action: DesktopAction) -> Option<Self> {
        let det = m[0] * m[3] - m[2] * m[1];
        if !det.is_finite() || det.abs() < 1e-8 || !center[0].is_finite() || !center[1].is_finite() {
            return None;
        }
        let k = 1.0 / det;
        Some(Self {
            center,
            inv: [m[3] * k, -m[1] * k, -m[2] * k, m[0] * k],
            half: [half[0].max(0.0), half[1].max(0.0)],
            action,
        })
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        let d = [p[0] - self.center[0], p[1] - self.center[1]];
        let lx = self.inv[0] * d[0] + self.inv[2] * d[1];
        let ly = self.inv[1] * d[0] + self.inv[3] * d[1];
        lx.abs() <= self.half[0] && ly.abs() <= self.half[1]
    }
}

/// Regiões de uma saída, em ordem de desenho (a última por cima).
#[derive(Debug, Default)]
pub struct HitMap {
    regions: Vec<HitRegion>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn push(&mut self, region: HitRegion) {
        self.regions.push(region);
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Região mais ao topo que contém `p` (px físicos locais da saída).
    pub fn hit_test(&self, p: [f32; 2]) -> Option<DesktopAction> {
        self.regions.iter().rev().find(|r| r.contains(p)).map(|r| r.action)
    }
}

/// Converte um toque global (desktop virtual) para coordenadas locais da saída em
/// `origin`/`size` (px físicos). `None` se o ponto está em outro monitor.
pub fn tap_to_local(tap: (i32, i32), origin: (i32, i32), size: (u32, u32)) -> Option<[f32; 2]> {
    let x = i64::from(tap.0) - i64::from(origin.0);
    let y = i64::from(tap.1) - i64::from(origin.1);
    if x < 0 || y < 0 || x >= i64::from(size.0) || y >= i64::from(size.1) {
        return None;
    }
    Some([x as f32 + 0.5, y as f32 + 0.5])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_region() {
        // 90°: largura local (x) vira vertical na tela.
        let r = HitRegion::new([100.0, 100.0], [0.0, 1.0, -1.0, 0.0], [40.0, 10.0], DesktopAction::MediaNext);
        let r = r.expect("região válida");
        assert!(r.contains([100.0, 135.0]));
        assert!(!r.contains([135.0, 100.0]));
    }

    #[test]
    fn topmost_wins() {
        let mut map = HitMap::default();
        let id = [1.0, 0.0, 0.0, 1.0];
        map.push(HitRegion::new([50.0, 50.0], id, [50.0, 50.0], DesktopAction::MediaPrev).expect("ok"));
        map.push(HitRegion::new([50.0, 50.0], id, [10.0, 10.0], DesktopAction::MediaPlayPause).expect("ok"));
        assert_eq!(map.hit_test([50.0, 50.0]), Some(DesktopAction::MediaPlayPause));
        assert_eq!(map.hit_test([5.0, 5.0]), Some(DesktopAction::MediaPrev));
        assert_eq!(tap_to_local((-10, 5), (0, 0), (100, 100)), None);
    }
}
