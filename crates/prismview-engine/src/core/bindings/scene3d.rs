//! Bindings reativos 3D (`ReactiveBinding3D` do Estúdio → objetos glTF nativos).
//!
//! Por quadro (`BindingSolver3d::solve`, chamado em `Scene3dRenderer::update` depois de avançar os
//! clipes e antes de `refresh_pose`):
//! 1. `AdditiveLayer::begin` em cada instância (zera o delta do quadro anterior);
//! 2. cada binding avança sua mola e acumula rotação/offset/escala/morph/brilho na camada;
//! 3. `commit_additive` marca a instância: `refresh_pose` então restaura a pose animada (desfaz)
//!    ou reamostra o clipe (se ele avançou) e reaplica a camada — sem deriva.
//!
//! Alvos são resolvidos por nome **uma vez** quando a cena/modelos mudam (`invalidate`); o quadro
//! só indexa vetores. Picking (hover/clique) é feito pelo renderer (`PickHit`) e passado para cá.

use std::f32::consts::TAU;

use glam::{EulerRot, Quat};
use serde::Deserialize;
use serde_json::Value;

use super::{or_default, spring_alpha, spring_step, Axis, BindingInput};
use crate::engine_core::scene3d::instance::ModelInstance;
use crate::engine_core::signals::{SignalKind, SignalSnapshot};

/// Nó-alvo especial que representa o modelo inteiro.
pub const ROOT_NODE: &str = "__ROOT__";

// ───────────────────────── Contrato JSON ─────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Binding3dSpec {
    id: String,
    enabled: bool,
    object_id: Option<String>,
    source: String,
    source_node: String,
    target_type: String,
    target_name: String,
    strength: f32,
    smoothing: f32,
    invert_x: bool,
    invert_y: bool,
    axis: Option<String>,
    invert_axis: bool,
}

impl Default for Binding3dSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            object_id: None,
            source: String::new(),
            source_node: "*".to_owned(),
            target_type: String::new(),
            target_name: String::new(),
            strength: 0.0,
            smoothing: 0.0,
            invert_x: false,
            invert_y: false,
            axis: None,
            invert_axis: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target3d {
    LookAt,
    PlayClip,
    EmissiveGlow,
    ScalePulse,
    MorphWeight,
    RotateSpin,
    RotateSwing,
    MoveOffset,
}

impl Target3d {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim() {
            "NODE_LOOK_AT" => Self::LookAt,
            "PLAY_CLIP" => Self::PlayClip,
            "EMISSIVE_GLOW" => Self::EmissiveGlow,
            "NODE_SCALE_PULSE" => Self::ScalePulse,
            "MORPH_WEIGHT" => Self::MorphWeight,
            "NODE_ROTATE_SPIN" => Self::RotateSpin,
            "NODE_ROTATE_SWING" => Self::RotateSwing,
            "NODE_MOVE_OFFSET" => Self::MoveOffset,
            _ => return None,
        })
    }

    /// Alvos de transformação que respondem a pulso de clique.
    fn is_motion(self) -> bool {
        matches!(self, Self::ScalePulse | Self::RotateSpin | Self::RotateSwing | Self::MoveOffset)
    }
}

// ───────────────────────── Estado / resolução ─────────────────────────

#[derive(Clone, Copy, Debug, Default)]
struct State {
    weight: f32,
    spin: f32,
    pulse: f32,
    yaw: f32,
    pitch: f32,
    toggled: bool,
    /// PLAY_CLIP: sinal estava acima do limiar no quadro anterior.
    active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NodeRef {
    #[default]
    Missing,
    Root,
    Node(u32),
}

#[derive(Clone, Debug, Default)]
struct Resolved {
    object: Option<usize>,
    node: NodeRef,
    all_materials: bool,
    materials: Vec<u32>,
    /// Índices em `Pose::weights`.
    morph: Vec<u32>,
    clip: Option<usize>,
    source_any: bool,
    source_nodes: Vec<u32>,
}

struct Binding3d {
    id: String,
    /// Dono de override de clipe (posição na lista).
    key: u32,
    input: BindingInput,
    target: Target3d,
    object_id: Option<String>,
    source_node: String,
    target_name: String,
    strength: f32,
    smoothing: f32,
    invert_x: bool,
    invert_y: bool,
    axis: Axis,
    sign: f32,
    state: State,
    res: Resolved,
}

/// Acesso do solver às instâncias carregadas do renderer (índice = objeto da cena).
pub trait InstanceAccess {
    fn object_count(&self) -> usize;
    fn object_index(&self, object_id: &str) -> Option<usize>;
    fn instance(&self, object: usize) -> Option<&ModelInstance>;
    fn instance_mut(&mut self, object: usize) -> Option<&mut ModelInstance>;
}

/// Resultado de picking: objeto da cena + nó glTF atingido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickHit {
    pub object: usize,
    pub node: u32,
}

// ───────────────────────── Solver ─────────────────────────

#[derive(Default)]
pub struct BindingSolver3d {
    bindings: Vec<Binding3d>,
    resolved: bool,
    demand: SignalKind,
    has_hover: bool,
    has_click: bool,
    hover: Option<PickHit>,
    animating: bool,
}

impl BindingSolver3d {
    pub fn new() -> Self {
        Self::default()
    }

    /// Carrega `Scene3DConfig.bindings` (JSON bruto). Entradas inválidas/desligadas são ignoradas;
    /// bindings com o mesmo `id` mantêm o estado das molas (edição ao vivo sem saltos).
    pub fn set_specs(&mut self, raw: &[Value]) {
        let old = std::mem::take(&mut self.bindings);
        let mut demand = SignalKind::NONE;
        let (mut has_hover, mut has_click) = (false, false);
        for value in raw {
            let Ok(spec) = Binding3dSpec::deserialize(value) else {
                continue;
            };
            if !spec.enabled {
                continue;
            }
            let (Some(input), Some(target)) =
                (BindingInput::parse(&spec.source), Target3d::parse(&spec.target_type))
            else {
                continue;
            };
            demand |= input.kind();
            has_hover |= input == BindingInput::HoverMesh;
            has_click |= input == BindingInput::ClickMesh;
            let state = old
                .iter()
                .find(|b| !spec.id.is_empty() && b.id == spec.id)
                .map(|b| b.state)
                .unwrap_or_default();
            let key = u32::try_from(self.bindings.len()).unwrap_or(u32::MAX);
            self.bindings.push(Binding3d {
                id: spec.id,
                key,
                input,
                target,
                object_id: spec.object_id.filter(|s| !s.trim().is_empty()),
                source_node: spec.source_node,
                target_name: spec.target_name,
                strength: spec.strength,
                smoothing: spec.smoothing,
                invert_x: spec.invert_x,
                invert_y: spec.invert_y,
                axis: Axis::parse(spec.axis.as_deref()),
                sign: if spec.invert_axis { -1.0 } else { 1.0 },
                state,
                res: Resolved::default(),
            });
        }
        self.demand = demand;
        self.has_hover = has_hover;
        self.has_click = has_click;
        self.hover = None;
        self.resolved = false;
    }

    /// Modelos mudaram (carregou/trocou): resolver os alvos de novo no próximo quadro.
    pub fn invalidate(&mut self) {
        self.resolved = false;
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Sinais que os bindings consomem (para `SignalBus::set_demand`).
    pub fn demand(&self) -> SignalKind {
        self.demand
    }

    pub fn wants_hover(&self) -> bool {
        self.has_hover
    }

    pub fn wants_click(&self) -> bool {
        self.has_click
    }

    pub fn set_hover(&mut self, hit: Option<PickHit>) {
        self.hover = hit;
    }

    /// Alguma mola/giro/pulso ainda se move → o loop precisa de mais quadros.
    pub fn is_animating(&self) -> bool {
        self.animating
    }

    fn resolve(&mut self, access: &impl InstanceAccess) {
        if self.resolved {
            return;
        }
        for b in &mut self.bindings {
            b.res = Resolved::default();
            // `bindingOwnerId` do Estúdio: `objectId` existente → esse objeto; vazio/desconhecido →
            // `objects[0]`. Dono ainda sem modelo = binding sem alvo até o próximo `invalidate`.
            let owner = b
                .object_id
                .as_deref()
                .and_then(|id| access.object_index(id))
                .or_else(|| (access.object_count() > 0).then_some(0));
            let object = owner.filter(|&i| access.instance(i).is_some());
            b.res.object = object;
            let Some(inst) = object.and_then(|i| access.instance(i)) else {
                continue;
            };
            let rig = &inst.model.rig;
            let name = b.target_name.trim();

            b.res.node = if name.is_empty() || name == ROOT_NODE {
                NodeRef::Root
            } else if let Some(n) = rig.node_by_name(name) {
                NodeRef::Node(n)
            } else {
                (0..rig.node_names.len())
                    .filter_map(|i| u32::try_from(i).ok())
                    .find(|&n| rig.node_matches(n, name))
                    .map_or(NodeRef::Missing, NodeRef::Node)
            };

            match b.target {
                Target3d::EmissiveGlow => {
                    b.res.all_materials = name.is_empty() || name == "*";
                    if !b.res.all_materials {
                        b.res.materials = rig
                            .material_names
                            .iter()
                            .enumerate()
                            .filter(|(_, n)| n.as_deref() == Some(name))
                            .filter_map(|(i, _)| u32::try_from(i).ok())
                            .collect();
                    }
                }
                Target3d::MorphWeight => {
                    let by_index: Option<usize> = name.parse().ok();
                    let pose = &rig.rest_pose;
                    for (node, &(start, count)) in pose.weight_range.iter().enumerate() {
                        let names = rig
                            .node_mesh
                            .get(node)
                            .copied()
                            .flatten()
                            .and_then(|m| rig.morph_names.get(m as usize));
                        for t in 0..count {
                            let hit = names.and_then(|n| n.get(t as usize)).is_some_and(|n| n == name)
                                || by_index == Some(t as usize);
                            if hit {
                                b.res.morph.push(start + t);
                            }
                        }
                    }
                }
                Target3d::PlayClip => b.res.clip = inst.clip_index(name),
                _ => {}
            }

            let source = b.source_node.trim();
            b.res.source_any = source.is_empty() || source == "*";
            if !b.res.source_any {
                b.res.source_nodes = (0..rig.node_names.len())
                    .filter_map(|i| u32::try_from(i).ok())
                    .filter(|&n| rig.node_matches(n, source))
                    .collect();
            }
        }
        self.resolved = true;
    }

    /// Clique (já convertido em `PickHit` pelo renderer). `true` se algum binding disparou.
    pub fn click(&mut self, access: &mut impl InstanceAccess, hit: PickHit) -> bool {
        if !self.has_click {
            return false;
        }
        self.resolve(access);
        let mut triggered = false;
        for b in &mut self.bindings {
            if b.input != BindingInput::ClickMesh || b.res.object != Some(hit.object) {
                continue;
            }
            let Some(inst) = access.instance_mut(hit.object) else {
                continue;
            };
            if !matches_source(&b.res, inst, hit.node) {
                continue;
            }
            triggered = true;
            match b.target {
                Target3d::PlayClip => {
                    if let Some(clip) = b.res.clip {
                        inst.play_override(clip, false, None);
                    }
                }
                Target3d::EmissiveGlow | Target3d::MorphWeight => b.state.toggled = !b.state.toggled,
                t if t.is_motion() => b.state.pulse = 1.0,
                _ => {}
            }
        }
        if triggered {
            self.animating = true;
        }
        triggered
    }

    /// Avalia todos os bindings para o quadro. `pointer` = cursor normalizado (0..1) sobre a
    /// camada. Devolve `true` se algo visível mudou. Sem alocação.
    pub fn solve(
        &mut self,
        access: &mut impl InstanceAccess,
        snap: &SignalSnapshot,
        pointer: Option<[f32; 2]>,
        dt: f32,
    ) -> bool {
        if self.bindings.is_empty() && !self.animating {
            return false;
        }
        self.resolve(access);
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };

        // 1. Desfaz: zera o delta do quadro anterior em todas as instâncias.
        for i in 0..access.object_count() {
            if let Some(inst) = access.instance_mut(i) {
                inst.additive_mut().begin();
            }
        }

        let (cx, cy) = pointer.map_or((0.0, 0.0), |p| ((p[0] - 0.5) * 2.0, (p[1] - 0.5) * 2.0));
        let hover = self.hover;
        let mut animating = false;

        // 2. Reaplica: cada binding acumula na camada aditiva da sua instância.
        for b in &mut self.bindings {
            let Some(object) = b.res.object else {
                continue;
            };
            let Some(inst) = access.instance_mut(object) else {
                continue;
            };
            match b.input {
                BindingInput::Signal(_) => {
                    let raw = b.input.scalar(snap);
                    let alpha = spring_alpha(b.smoothing, 0.18, dt);
                    animating |= apply_scalar(b, inst, raw, alpha, dt);
                }
                BindingInput::CursorLook => {
                    if b.target == Target3d::LookAt {
                        animating |= apply_look(b, inst, cx, cy, dt);
                    }
                }
                BindingInput::HoverMesh => {
                    let hovered = hover
                        .is_some_and(|h| h.object == object && matches_source(&b.res, inst, h.node));
                    let alpha = spring_alpha(b.smoothing, 0.12, dt);
                    animating |= apply_scalar(b, inst, if hovered { 1.0 } else { 0.0 }, alpha, dt);
                }
                BindingInput::ClickMesh => {
                    if b.target.is_motion() {
                        animating |= apply_click_pulse(b, inst, dt);
                    } else if matches!(b.target, Target3d::EmissiveGlow | Target3d::MorphWeight) {
                        let alpha = spring_alpha(b.smoothing, 0.12, dt);
                        let raw = if b.state.toggled { 1.0 } else { 0.0 };
                        animating |= apply_scalar(b, inst, raw, alpha, dt);
                    }
                }
            }
        }

        // 3. Fecha: instâncias cujo delta mudou refazem a pose (restaura → reaplica).
        let mut changed = false;
        for i in 0..access.object_count() {
            if let Some(inst) = access.instance_mut(i) {
                changed |= inst.commit_additive();
            }
        }
        self.animating = animating;
        changed || animating
    }
}

// ───────────────────────── Aplicação ─────────────────────────

/// `matchesMeshTarget`: o nó atingido ou um ancestral tem o nome de `sourceNode` ("*" = qualquer).
fn matches_source(res: &Resolved, inst: &ModelInstance, hit_node: u32) -> bool {
    if res.source_any {
        return true;
    }
    let mut cursor = Some(hit_node);
    let mut guard = 0usize;
    while let Some(n) = cursor {
        if res.source_nodes.contains(&n) {
            return true;
        }
        guard += 1;
        if guard > 4096 {
            break;
        }
        cursor = inst.parent_of(n);
    }
    false
}

fn add_rotation(inst: &mut ModelInstance, node: NodeRef, q: Quat) {
    match node {
        NodeRef::Root => {
            let layer = inst.additive_mut();
            layer.root_rot *= q;
        }
        NodeRef::Node(n) => inst.additive_mut().add_rotation(n, q),
        NodeRef::Missing => {}
    }
}

fn add_axis_rotation(inst: &mut ModelInstance, node: NodeRef, axis: Axis, angle: f32) {
    if angle.is_finite() && angle != 0.0 {
        add_rotation(inst, node, Quat::from_axis_angle(axis.unit(), angle));
    }
}

/// Offset em unidades de exibição (modelo normalizado), convertido para o espaço do nó.
fn add_axis_offset(inst: &mut ModelInstance, node: NodeRef, axis: Axis, amount: f32) {
    if !amount.is_finite() || amount == 0.0 {
        return;
    }
    match node {
        NodeRef::Root => inst.additive_mut().root_pos += axis.unit() * amount,
        NodeRef::Node(n) => {
            let k = inst.display_to_node_units(n);
            inst.additive_mut().add_offset(n, axis.unit() * (amount * k));
        }
        NodeRef::Missing => {}
    }
}

fn add_scale(inst: &mut ModelInstance, node: NodeRef, ratio: f32) {
    if !ratio.is_finite() || ratio == 0.0 {
        return;
    }
    match node {
        NodeRef::Root => inst.additive_mut().root_scale += ratio,
        NodeRef::Node(n) => inst.additive_mut().add_scale(n, ratio),
        NodeRef::Missing => {}
    }
}

/// `applyScalarTarget` do Estúdio. Devolve `true` se o binding ainda se move.
fn apply_scalar(b: &mut Binding3d, inst: &mut ModelInstance, raw: f32, alpha: f32, dt: f32) -> bool {
    if b.target == Target3d::PlayClip {
        let active = raw >= 0.35;
        if active && !b.state.active {
            if let Some(clip) = b.res.clip {
                inst.play_override(clip, true, Some(b.key));
            }
        } else if !active && b.state.active && inst.override_owner() == Some(b.key) {
            inst.return_to_base();
        }
        b.state.active = active;
        return false; // o próprio clipe mantém o loop acordado
    }

    let moved = spring_step(&mut b.state.weight, raw, alpha);
    let w = b.state.weight;
    let spinning = b.target == Target3d::RotateSpin && w.abs() > 1e-4;
    let moving = moved || spinning;
    if moving {
        inst.additive_mut().mark_moving();
    }

    match b.target {
        Target3d::ScalePulse => add_scale(inst, b.res.node, w * or_default(b.strength, 0.16)),
        Target3d::EmissiveGlow => {
            let amount = w * or_default(b.strength, 1.4);
            if amount != 0.0 {
                let layer = inst.additive_mut();
                if b.res.all_materials {
                    layer.add_glow_all(amount);
                } else {
                    for &m in &b.res.materials {
                        layer.add_glow(m, amount);
                    }
                }
            }
        }
        Target3d::MorphWeight => {
            let v = w * or_default(b.strength, 1.0).clamp(0.0, 1.0);
            if v > 0.0 {
                let layer = inst.additive_mut();
                for &i in &b.res.morph {
                    layer.max_morph(i, v);
                }
            }
        }
        Target3d::RotateSpin => {
            if spinning {
                b.state.spin = (b.state.spin + w * or_default(b.strength, 2.0) * dt) % TAU;
            }
            add_axis_rotation(inst, b.res.node, b.axis, b.state.spin * b.sign);
        }
        Target3d::RotateSwing => {
            let angle = w * or_default(b.strength, 25.0).to_radians() * b.sign;
            add_axis_rotation(inst, b.res.node, b.axis, angle);
        }
        Target3d::MoveOffset => {
            add_axis_offset(inst, b.res.node, b.axis, w * or_default(b.strength, 0.15) * b.sign);
        }
        Target3d::LookAt | Target3d::PlayClip => {}
    }
    moving
}

/// `CURSOR_LOOK` → `NODE_LOOK_AT`: yaw/pitch amortecidos (Euler YXZ, pitch × 0,72).
fn apply_look(b: &mut Binding3d, inst: &mut ModelInstance, cx: f32, cy: f32, dt: f32) -> bool {
    let max_rad = or_default(b.strength, 20.0).clamp(0.0, 75.0).to_radians();
    let t_yaw = if b.invert_x { -cx } else { cx } * max_rad;
    let t_pitch = if b.invert_y { -cy } else { cy } * max_rad * 0.72;
    let alpha = spring_alpha(b.smoothing, 0.12, dt);
    let (prev_yaw, prev_pitch) = (b.state.yaw, b.state.pitch);
    b.state.yaw += (t_yaw - b.state.yaw) * alpha;
    b.state.pitch += (t_pitch - b.state.pitch) * alpha;
    if (t_yaw - b.state.yaw).abs() < 0.001 {
        b.state.yaw = t_yaw;
    }
    if (t_pitch - b.state.pitch).abs() < 0.001 {
        b.state.pitch = t_pitch;
    }
    let moving = b.state.yaw != prev_yaw || b.state.pitch != prev_pitch;
    if moving {
        inst.additive_mut().mark_moving();
    }
    if b.state.yaw.abs() > 1e-6 || b.state.pitch.abs() > 1e-6 {
        let q = Quat::from_euler(EulerRot::YXZ, b.state.yaw, b.state.pitch, 0.0);
        add_rotation(inst, b.res.node, q);
    }
    moving
}

/// Pulso de clique com decaimento `exp(-max(2, smoothing·45)·dt)`.
fn apply_click_pulse(b: &mut Binding3d, inst: &mut ModelInstance, dt: f32) -> bool {
    let mut moving = false;
    if b.state.pulse > 0.002 {
        b.state.pulse *= (-(or_default(b.smoothing, 0.15) * 45.0).max(2.0) * dt).exp();
        moving = true;
    } else if b.state.pulse != 0.0 {
        b.state.pulse = 0.0;
        moving = true; // último quadro: some o offset residual
    }
    if moving {
        inst.additive_mut().mark_moving();
    }
    let p = b.state.pulse;
    if p <= 0.001 {
        return moving;
    }
    match b.target {
        Target3d::ScalePulse => add_scale(inst, b.res.node, p * or_default(b.strength, 0.16)),
        Target3d::RotateSpin => {
            b.state.spin = (b.state.spin + p * or_default(b.strength, 4.0) * dt) % TAU;
            add_axis_rotation(inst, b.res.node, b.axis, b.state.spin * b.sign);
        }
        Target3d::RotateSwing => {
            let angle = p * or_default(b.strength, 25.0).to_radians() * b.sign;
            add_axis_rotation(inst, b.res.node, b.axis, angle);
        }
        Target3d::MoveOffset => {
            add_axis_offset(inst, b.res.node, b.axis, p * or_default(b.strength, 0.15) * b.sign);
        }
        _ => {}
    }
    moving
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_studio_binding() {
        let v: Value = serde_json::json!({
            "id": "b1", "enabled": true, "source": "AUDIO_BASS", "sourceNode": "*",
            "targetType": "NODE_ROTATE_SWING", "targetName": "Jaw", "strength": 30,
            "smoothing": 0.2, "axis": "X", "invertAxis": true
        });
        let mut s = BindingSolver3d::new();
        s.set_specs(&[v]);
        assert_eq!(s.bindings.len(), 1);
        assert_eq!(s.bindings[0].axis, Axis::X);
        assert_eq!(s.bindings[0].sign, -1.0);
        assert!(s.demand().contains(SignalKind::AUDIO));
    }

    #[test]
    fn spring_settles_exactly() {
        let mut v = 0.0;
        for _ in 0..600 {
            spring_step(&mut v, 1.0, spring_alpha(0.18, 0.18, 1.0 / 60.0));
        }
        assert_eq!(v, 1.0);
    }
}
