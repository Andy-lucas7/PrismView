//! Animação de nós glTF na CPU (pose local T/R/S + pesos de morph), sem alocação por quadro.
//!
//! Fluxo por quadro (determinístico, drift-free — PLANO_NATIVO §1.5):
//! 1. `Pose::reset_to_rest` copia a pose de repouso (nada acumula entre quadros);
//! 2. `sample_clip` sobrescreve os canais animados no tempo `t`;
//! 3. `Pose::store_animated` guarda a pose amostrada (sem bindings);
//! 4. `AdditiveLayer::apply` soma os offsets dos bindings reativos (Fase 4) por cima;
//! 5. `Pose::world_matrices` resolve a hierarquia (ordem pai→filho pré-calculada).
//!
//! Quando só os bindings mudam (clipe parado/sem clipe), `Pose::restore_animated` **desfaz** o
//! offset do quadro anterior copiando de volta a pose animada guardada — nada é invertido
//! numericamente (sem `q⁻¹`/subtração), então não há deriva acumulada nem após milhões de quadros.
//!
//! Todos os `Vec` são dimensionados em `Pose::new` (carregamento do modelo) e só são reescritos.

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::engine_core::assets::{AnimPath, AnimationChannel, AnimationClip, Interpolation, ModelAsset};

// ───────────────────────── Pose ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodePose {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl NodePose {
    #[inline]
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// Pose de todos os nós de um modelo + pesos de morph por nó (achatados). O `GpuModel` guarda a
/// pose de repouso; cada instância clona a sua no carregamento.
#[derive(Clone)]
pub struct Pose {
    rest: Vec<NodePose>,
    pub nodes: Vec<NodePose>,
    rest_weights: Vec<f32>,
    /// Pesos de morph de todos os nós com malha morfável; faixa de cada nó em `weight_range`.
    pub weights: Vec<f32>,
    /// Pose amostrada do clipe **antes** dos bindings (base limpa para desfazer offsets).
    animated: Vec<NodePose>,
    animated_weights: Vec<f32>,
    /// `(início, quantidade)` em `weights` por nó (quantidade 0 = nó sem morph).
    pub weight_range: Vec<(u32, u32)>,
    /// Índices dos nós em ordem pai→filho (todos os nós, inclusive fora da cena).
    order: Vec<u32>,
    parent: Vec<Option<u32>>,
    /// Nó alcançável a partir das raízes da cena padrão (só esses são desenhados).
    pub in_scene: Vec<bool>,
}

impl Pose {
    /// Monta a pose de repouso. Aloca (chamado no carregamento, nunca no quadro).
    pub fn new(asset: &ModelAsset) -> Self {
        let count = asset.nodes.len();
        let rest: Vec<NodePose> = asset
            .nodes
            .iter()
            .map(|n| NodePose {
                translation: n.translation,
                rotation: n.rotation,
                scale: n.scale,
            })
            .collect();

        let mut rest_weights = Vec::new();
        let mut weight_range = Vec::with_capacity(count);
        for node in &asset.nodes {
            let mesh = node.mesh.and_then(|m| asset.meshes.get(m as usize));
            let targets = mesh.map_or(0, |m| m.morph_target_count());
            let start = u32::try_from(rest_weights.len()).unwrap_or(u32::MAX);
            for t in 0..targets {
                let w = node
                    .weights
                    .get(t)
                    .or_else(|| mesh.and_then(|m| m.weights.get(t)))
                    .copied()
                    .filter(|w| w.is_finite())
                    .unwrap_or(0.0);
                rest_weights.push(w);
            }
            weight_range.push((start, u32::try_from(targets).unwrap_or(0)));
        }

        // Ordem pai→filho por profundidade (o loader já quebrou ciclos).
        let parent: Vec<Option<u32>> = asset.nodes.iter().map(|n| n.parent).collect();
        let mut depth = vec![0u32; count];
        for (i, d) in depth.iter_mut().enumerate() {
            let mut cursor = parent[i];
            let mut steps = 0u32;
            while let Some(p) = cursor {
                steps += 1;
                if steps as usize > count {
                    break;
                }
                cursor = parent.get(p as usize).copied().flatten();
            }
            *d = steps;
        }
        let mut order: Vec<u32> = (0..count).filter_map(|i| u32::try_from(i).ok()).collect();
        order.sort_by_key(|&i| depth[i as usize]);

        let mut in_scene = vec![false; count];
        let mut stack: Vec<u32> = asset.roots.clone();
        while let Some(i) = stack.pop() {
            let Some(flag) = in_scene.get_mut(i as usize) else {
                continue;
            };
            if *flag {
                continue;
            }
            *flag = true;
            if let Some(node) = asset.nodes.get(i as usize) {
                stack.extend_from_slice(&node.children);
            }
        }

        Self {
            nodes: rest.clone(),
            animated: rest.clone(),
            rest,
            weights: rest_weights.clone(),
            animated_weights: rest_weights.clone(),
            rest_weights,
            weight_range,
            order,
            parent,
            in_scene,
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Volta à pose de repouso (sem alocar).
    pub fn reset_to_rest(&mut self) {
        self.nodes.copy_from_slice(&self.rest);
        self.weights.copy_from_slice(&self.rest_weights);
    }

    /// Guarda a pose atual (recém-amostrada) como base limpa dos bindings. Sem alocar.
    pub fn store_animated(&mut self) {
        self.animated.copy_from_slice(&self.nodes);
        self.animated_weights.copy_from_slice(&self.weights);
    }

    /// Desfaz os offsets do quadro anterior: volta à última pose amostrada. Sem alocar.
    pub fn restore_animated(&mut self) {
        self.nodes.copy_from_slice(&self.animated);
        self.weights.copy_from_slice(&self.animated_weights);
    }

    #[inline]
    pub fn parent_of(&self, node: usize) -> Option<u32> {
        self.parent.get(node).copied().flatten()
    }

    pub fn weight_count(&self) -> usize {
        self.weights.len()
    }

    #[allow(dead_code)] // API reservada (ainda sem chamador)
    pub fn node_weights(&self, node: usize) -> &[f32] {
        match self.weight_range.get(node) {
            Some(&(start, len)) => {
                let (s, l) = (start as usize, len as usize);
                self.weights.get(s..s + l).unwrap_or(&[])
            }
            None => &[],
        }
    }

    /// Matrizes de mundo (espaço do modelo) em `out` (mesmo tamanho que os nós).
    pub fn world_matrices(&self, out: &mut [Mat4]) {
        for &i in &self.order {
            let i = i as usize;
            let (Some(local), true) = (self.nodes.get(i), i < out.len()) else {
                continue;
            };
            let local = local.matrix();
            out[i] = match self.parent.get(i).copied().flatten() {
                Some(p) if (p as usize) < out.len() => out[p as usize] * local,
                _ => local,
            };
        }
    }
}

// ───────────────────────── Amostragem ─────────────────────────

/// Localiza o par de chaves em torno de `t`: `(i, j, u)` com `u` ∈ [0, 1] e `dt` entre as chaves.
#[inline]
fn locate(inputs: &[f32], t: f32) -> (usize, usize, f32, f32) {
    let n = inputs.len();
    if n <= 1 || t <= inputs[0] {
        return (0, 0, 0.0, 0.0);
    }
    if t >= inputs[n - 1] {
        return (n - 1, n - 1, 0.0, 0.0);
    }
    // partition_point: primeira chave > t; está em 1..n-1 pelos testes acima.
    let j = inputs.partition_point(|&x| x <= t).clamp(1, n - 1);
    let i = j - 1;
    let dt = inputs[j] - inputs[i];
    let u = if dt > 1e-9 { ((t - inputs[i]) / dt).clamp(0.0, 1.0) } else { 0.0 };
    (i, j, u, dt)
}

/// Valor do componente `c` da chave `k` (pula as tangentes em CUBICSPLINE).
#[inline]
fn key_value(ch: &AnimationChannel, k: usize, c: usize) -> f32 {
    let comps = ch.components;
    let idx = match ch.interpolation {
        Interpolation::CubicSpline => (k * 3 + 1) * comps + c,
        _ => k * comps + c,
    };
    ch.outputs.get(idx).copied().unwrap_or(0.0)
}

/// Tangente de entrada (`slot` 0) ou saída (`slot` 2) da chave `k` em CUBICSPLINE.
#[inline]
fn key_tangent(ch: &AnimationChannel, k: usize, slot: usize, c: usize) -> f32 {
    ch.outputs
        .get((k * 3 + slot) * ch.components + c)
        .copied()
        .unwrap_or(0.0)
}

/// Componente `c` do canal no tempo localizado (STEP/LINEAR/CUBICSPLINE escalar).
#[inline]
fn sample_component(ch: &AnimationChannel, i: usize, j: usize, u: f32, dt: f32, c: usize) -> f32 {
    let a = key_value(ch, i, c);
    if i == j {
        return a;
    }
    match ch.interpolation {
        Interpolation::Step => a,
        Interpolation::Linear => a + (key_value(ch, j, c) - a) * u,
        Interpolation::CubicSpline => {
            let b = key_value(ch, j, c);
            let out_tan = key_tangent(ch, i, 2, c) * dt;
            let in_tan = key_tangent(ch, j, 0, c) * dt;
            let u2 = u * u;
            let u3 = u2 * u;
            (2.0 * u3 - 3.0 * u2 + 1.0) * a
                + (u3 - 2.0 * u2 + u) * out_tan
                + (-2.0 * u3 + 3.0 * u2) * b
                + (u3 - u2) * in_tan
        }
    }
}

fn sample_vec3(ch: &AnimationChannel, i: usize, j: usize, u: f32, dt: f32) -> Vec3 {
    Vec3::new(
        sample_component(ch, i, j, u, dt, 0),
        sample_component(ch, i, j, u, dt, 1),
        sample_component(ch, i, j, u, dt, 2),
    )
}

fn key_quat(ch: &AnimationChannel, k: usize) -> Quat {
    let q = Vec4::new(
        key_value(ch, k, 0),
        key_value(ch, k, 1),
        key_value(ch, k, 2),
        key_value(ch, k, 3),
    );
    Quat::from_vec4(q).normalize()
}

fn sample_quat(ch: &AnimationChannel, i: usize, j: usize, u: f32, dt: f32) -> Quat {
    let q = match ch.interpolation {
        Interpolation::Step => key_quat(ch, i),
        Interpolation::Linear if i != j => key_quat(ch, i).slerp(key_quat(ch, j), u),
        Interpolation::Linear => key_quat(ch, i),
        Interpolation::CubicSpline => Quat::from_vec4(Vec4::new(
            sample_component(ch, i, j, u, dt, 0),
            sample_component(ch, i, j, u, dt, 1),
            sample_component(ch, i, j, u, dt, 2),
            sample_component(ch, i, j, u, dt, 3),
        )),
    };
    let q = q.normalize();
    if q.is_finite() {
        q
    } else {
        Quat::IDENTITY
    }
}

/// Aplica todos os canais do clipe no tempo `t` (s) sobre `pose` (que deve vir de `reset_to_rest`).
pub fn sample_clip(clip: &AnimationClip, t: f32, pose: &mut Pose) {
    for ch in &clip.channels {
        let node = ch.node as usize;
        if node >= pose.nodes.len() || ch.inputs.is_empty() {
            continue;
        }
        let (i, j, u, dt) = locate(&ch.inputs, t);
        match ch.path {
            AnimPath::Translation => pose.nodes[node].translation = sample_vec3(ch, i, j, u, dt),
            AnimPath::Scale => pose.nodes[node].scale = sample_vec3(ch, i, j, u, dt),
            AnimPath::Rotation => pose.nodes[node].rotation = sample_quat(ch, i, j, u, dt),
            AnimPath::Weights => {
                let Some(&(start, len)) = pose.weight_range.get(node) else {
                    continue;
                };
                let n = (len as usize).min(ch.components);
                for c in 0..n {
                    if let Some(w) = pose.weights.get_mut(start as usize + c) {
                        *w = sample_component(ch, i, j, u, dt, c);
                    }
                }
            }
        }
    }
}

// ───────────────────────── Camada aditiva (bindings) ─────────────────────────

/// Offsets dos bindings reativos de um quadro, aplicados sobre a pose animada.
///
/// Reescrita do zero a cada quadro (`begin` → acumula → `finish`): o valor nunca realimenta a
/// pose base, por isso o pipeline é drift-free. Só os nós/pesos tocados são zerados no `begin`
/// (listas `nodes`/`morphs`), então o custo é O(bindings), não O(nós). Tudo pré-alocado em `new`.
#[derive(Clone, Debug)]
pub struct AdditiveLayer {
    rot: Vec<Quat>,
    pos: Vec<Vec3>,
    scale: Vec<f32>,
    node_mark: Vec<bool>,
    nodes: Vec<u32>,
    morph: Vec<f32>,
    morph_mark: Vec<bool>,
    morphs: Vec<u32>,
    /// Brilho emissivo extra por material da instância.
    glow: Vec<f32>,
    /// Transformação extra do modelo inteiro (`__ROOT__`), em torno do centro normalizado.
    pub root_rot: Quat,
    pub root_pos: Vec3,
    pub root_scale: f32,
    moving: bool,
    was_active: bool,
}

impl AdditiveLayer {
    pub fn new(node_count: usize, weight_count: usize, material_count: usize) -> Self {
        Self {
            rot: vec![Quat::IDENTITY; node_count],
            pos: vec![Vec3::ZERO; node_count],
            scale: vec![0.0; node_count],
            node_mark: vec![false; node_count],
            nodes: Vec::with_capacity(node_count),
            morph: vec![0.0; weight_count],
            morph_mark: vec![false; weight_count],
            morphs: Vec::with_capacity(weight_count),
            glow: vec![0.0; material_count],
            root_rot: Quat::IDENTITY,
            root_pos: Vec3::ZERO,
            root_scale: 0.0,
            moving: false,
            was_active: false,
        }
    }

    /// Zera os offsets do quadro anterior (só os tocados).
    pub fn begin(&mut self) {
        self.was_active = self.is_active();
        for &n in &self.nodes {
            let n = n as usize;
            if let Some(r) = self.rot.get_mut(n) {
                *r = Quat::IDENTITY;
            }
            if let Some(p) = self.pos.get_mut(n) {
                *p = Vec3::ZERO;
            }
            if let Some(s) = self.scale.get_mut(n) {
                *s = 0.0;
            }
            if let Some(m) = self.node_mark.get_mut(n) {
                *m = false;
            }
        }
        self.nodes.clear();
        for &i in &self.morphs {
            let i = i as usize;
            if let Some(v) = self.morph.get_mut(i) {
                *v = 0.0;
            }
            if let Some(m) = self.morph_mark.get_mut(i) {
                *m = false;
            }
        }
        self.morphs.clear();
        self.glow.iter_mut().for_each(|g| *g = 0.0);
        self.root_rot = Quat::IDENTITY;
        self.root_pos = Vec3::ZERO;
        self.root_scale = 0.0;
        self.moving = false;
    }

    #[inline]
    fn touch(&mut self, node: u32) -> Option<usize> {
        let n = node as usize;
        let mark = self.node_mark.get_mut(n)?;
        if !*mark {
            *mark = true;
            self.nodes.push(node);
        }
        Some(n)
    }

    /// Algum binding ainda está em movimento (mola, giro, pulso) → refazer a pose.
    #[inline]
    pub fn mark_moving(&mut self) {
        self.moving = true;
    }

    /// Rotação local extra (pós-multiplicada, no espaço do nó, como `node.quaternion.multiply`).
    pub fn add_rotation(&mut self, node: u32, q: Quat) {
        if let Some(n) = self.touch(node) {
            if let Some(r) = self.rot.get_mut(n) {
                *r *= q;
            }
        }
    }

    /// Deslocamento extra em unidades locais do nó.
    pub fn add_offset(&mut self, node: u32, v: Vec3) {
        if let Some(n) = self.touch(node) {
            if let Some(p) = self.pos.get_mut(n) {
                *p += v;
            }
        }
    }

    /// Escala relativa extra (`0.1` = +10 %).
    pub fn add_scale(&mut self, node: u32, ratio: f32) {
        if let Some(n) = self.touch(node) {
            if let Some(s) = self.scale.get_mut(n) {
                *s += ratio;
            }
        }
    }

    /// Peso de morph mínimo (`max` entre bindings e com a animação, como no Estúdio).
    pub fn max_morph(&mut self, weight_index: u32, value: f32) {
        let i = weight_index as usize;
        let (Some(v), Some(m)) = (self.morph.get_mut(i), self.morph_mark.get_mut(i)) else {
            return;
        };
        if *m {
            *v = v.max(value);
        } else {
            *m = true;
            *v = value;
            self.morphs.push(weight_index);
        }
    }

    pub fn add_glow(&mut self, material: u32, amount: f32) {
        if let Some(g) = self.glow.get_mut(material as usize) {
            *g += amount;
        }
    }

    pub fn add_glow_all(&mut self, amount: f32) {
        self.glow.iter_mut().for_each(|g| *g += amount);
    }

    #[inline]
    pub fn glow(&self, material: u32) -> f32 {
        self.glow.get(material as usize).copied().unwrap_or(0.0)
    }

    pub fn is_active(&self) -> bool {
        !self.nodes.is_empty()
            || !self.morphs.is_empty()
            || self.root_scale != 0.0
            || self.root_pos != Vec3::ZERO
            || self.root_rot != Quat::IDENTITY
            || self.glow.iter().any(|g| *g != 0.0)
    }

    /// Fecha o quadro. `true` se o resultado muda (algo se moveu ou a camada acabou de zerar).
    pub fn finish(&mut self) -> bool {
        self.moving || self.was_active != self.is_active()
    }

    /// Tem offsets de nó/morph (exige refazer a pose); raiz e brilho não tocam a pose.
    pub fn touches_pose(&self) -> bool {
        !self.nodes.is_empty() || !self.morphs.is_empty()
    }

    /// Matriz extra do modelo inteiro (`__ROOT__`).
    pub fn root_matrix(&self) -> Mat4 {
        if self.root_scale == 0.0 && self.root_pos == Vec3::ZERO && self.root_rot == Quat::IDENTITY {
            return Mat4::IDENTITY;
        }
        let s = (1.0 + self.root_scale).max(0.05);
        Mat4::from_scale_rotation_translation(Vec3::splat(s), self.root_rot.normalize(), self.root_pos)
    }

    /// Soma os offsets na pose (já restaurada/amostrada). Sem alocar.
    pub fn apply(&self, pose: &mut Pose) {
        for &n in &self.nodes {
            let n = n as usize;
            let (Some(r), Some(p), Some(s)) = (self.rot.get(n), self.pos.get(n), self.scale.get(n)) else {
                continue;
            };
            let Some(node) = pose.nodes.get_mut(n) else {
                continue;
            };
            if *r != Quat::IDENTITY {
                node.rotation = (node.rotation * *r).normalize();
            }
            node.translation += *p;
            if *s != 0.0 {
                node.scale *= (1.0 + *s).max(0.05);
            }
        }
        for &i in &self.morphs {
            let i = i as usize;
            if let (Some(v), Some(w)) = (self.morph.get(i), pose.weights.get_mut(i)) {
                *w = w.max(v.clamp(0.0, 1.0));
            }
        }
    }
}

// ───────────────────────── Player ─────────────────────────

/// Estado de reprodução de um clipe (equivalente ao `AnimationMixer` + action base do Estúdio).
#[derive(Clone, Copy, Debug)]
pub struct ClipPlayer {
    pub clip: Option<usize>,
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
    /// Clipe não-loop chegou ao fim (pose fica presa no último quadro).
    pub finished: bool,
}

impl Default for ClipPlayer {
    fn default() -> Self {
        Self {
            clip: None,
            time: 0.0,
            speed: 1.0,
            looping: true,
            finished: false,
        }
    }
}

impl ClipPlayer {
    /// Escolhe o clipe como o Estúdio: nome exato, senão o primeiro; `"__NONE__"` desliga.
    pub fn select(clips: &[AnimationClip], clip_name: &str) -> Option<usize> {
        if clip_name == "__NONE__" || clips.is_empty() {
            return None;
        }
        let by_name = clips
            .iter()
            .position(|c| c.name.as_deref() == Some(clip_name));
        Some(by_name.unwrap_or(0))
    }

    /// Está avançando o tempo (precisa de quadros contínuos).
    pub fn is_playing(&self) -> bool {
        self.clip.is_some() && self.speed.abs() > 1e-4 && !self.finished
    }

    /// Avança `dt` segundos. Devolve `true` se o tempo mudou (pose precisa ser refeita).
    pub fn advance(&mut self, dt: f32, duration: f32) -> bool {
        if !self.is_playing() || !dt.is_finite() || dt <= 0.0 {
            return false;
        }
        // Mesma regra do Estúdio: mixer.update(dt * max(0.05, speed)).
        let step = dt * self.speed.max(0.05);
        let before = self.time;
        if duration <= 1e-6 {
            self.time = 0.0;
            self.finished = !self.looping;
            return before != 0.0;
        }
        self.time += step;
        if self.looping {
            self.time = self.time.rem_euclid(duration);
        } else if self.time >= duration {
            self.time = duration;
            self.finished = true;
        }
        self.time != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(interp: Interpolation, inputs: Vec<f32>, outputs: Vec<f32>, comps: usize) -> AnimationChannel {
        AnimationChannel {
            node: 0,
            path: AnimPath::Translation,
            interpolation: interp,
            inputs,
            outputs,
            components: comps,
        }
    }

    #[test]
    fn linear_interpolates_and_clamps() {
        let ch = channel(Interpolation::Linear, vec![0.0, 1.0], vec![0.0, 0.0, 0.0, 2.0, 4.0, 6.0], 3);
        let (i, j, u, dt) = locate(&ch.inputs, 0.5);
        assert_eq!(sample_vec3(&ch, i, j, u, dt), Vec3::new(1.0, 2.0, 3.0));
        let (i, j, u, dt) = locate(&ch.inputs, 9.0);
        assert_eq!(sample_vec3(&ch, i, j, u, dt), Vec3::new(2.0, 4.0, 6.0));
    }

    #[test]
    fn step_holds_previous_key() {
        let ch = channel(Interpolation::Step, vec![0.0, 1.0], vec![1.0, 1.0, 1.0, 5.0, 5.0, 5.0], 3);
        let (i, j, u, dt) = locate(&ch.inputs, 0.99);
        assert_eq!(sample_vec3(&ch, i, j, u, dt), Vec3::ONE);
    }

    #[test]
    fn cubic_hits_keys() {
        // [in, value, out] por chave, 1 componente.
        let ch = channel(
            Interpolation::CubicSpline,
            vec![0.0, 2.0],
            vec![0.0, 1.0, 0.0, 0.0, 3.0, 0.0],
            1,
        );
        let (i, j, u, dt) = locate(&ch.inputs, 0.0);
        assert!((sample_component(&ch, i, j, u, dt, 0) - 1.0).abs() < 1e-6);
        let (i, j, u, dt) = locate(&ch.inputs, 2.0);
        assert!((sample_component(&ch, i, j, u, dt, 0) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn player_loops_and_clamps() {
        let mut p = ClipPlayer {
            clip: Some(0),
            ..ClipPlayer::default()
        };
        p.advance(2.5, 1.0);
        assert!((p.time - 0.5).abs() < 1e-5);
        p.looping = false;
        p.advance(5.0, 1.0);
        assert!(p.finished && (p.time - 1.0).abs() < 1e-6);
        assert!(!p.advance(1.0, 1.0));
    }
}
