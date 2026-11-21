// Skinning + morph targets na GPU (core/scene3d/instance.rs). Um dispatch por (nó, primitiva)
// deformável; parâmetros por job via uniform com offset dinâmico (escritos 1× na criação da
// instância). Só roda quando a pose mudou.
//
// Ordem glTF: morph (espaço da malha) → skin (matrizes de joint = mundo_joint · inverseBind).
// Saída no mesmo layout do stream 0 (`DeformVertex`), usada direto como vertex buffer.

struct Vtx {
    position: vec4<f32>,
    normal: vec4<f32>,
    tangent: vec4<f32>,
};

struct SkinVertex {
    joints: vec2<u32>, // 4 × u16 empacotados
    w01: vec2<f32>,
    w23: vec2<f32>,
};

struct Job {
    src_first: u32,
    out_first: u32,
    count: u32,
    joint_base: u32,
    joint_count: u32,
    morph_base: u32,
    morph_count: u32,
    weight_base: u32,
    flags: u32, // bit 0 = skin
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> job: Job;
@group(0) @binding(1) var<storage, read> src: array<Vtx>;
@group(0) @binding(2) var<storage, read> skin: array<SkinVertex>;
@group(0) @binding(3) var<storage, read> morph: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> joints: array<mat4x4<f32>>;
@group(0) @binding(5) var<storage, read> weights: array<f32>;
@group(0) @binding(6) var<storage, read_write> dst: array<Vtx>;

fn joint(index: u32) -> mat4x4<f32> {
    let j = min(index, max(job.joint_count, 1u) - 1u);
    return joints[job.joint_base + j];
}

@compute @workgroup_size(64, 1, 1)
fn cs_deform(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= job.count {
        return;
    }
    let v = src[job.src_first + i];
    var p = v.position.xyz;
    var n = v.normal.xyz;
    var t = v.tangent.xyz;

    for (var k = 0u; k < job.morph_count; k = k + 1u) {
        let w = weights[job.weight_base + k];
        if w != 0.0 {
            let e = (job.morph_base + k * job.count + i) * 2u;
            p = p + morph[e].xyz * w;
            n = n + morph[e + 1u].xyz * w;
        }
    }

    if (job.flags & 1u) != 0u && job.joint_count > 0u {
        let s = skin[job.src_first + i];
        var w = vec4<f32>(s.w01, s.w23);
        let total = w.x + w.y + w.z + w.w;
        if total > 1e-6 {
            w = w / total;
        } else {
            w = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        }
        let m = joint(s.joints.x & 0xffffu) * w.x
              + joint(s.joints.x >> 16u) * w.y
              + joint(s.joints.y & 0xffffu) * w.z
              + joint(s.joints.y >> 16u) * w.w;
        p = (m * vec4<f32>(p, 1.0)).xyz;
        // Sem escala não uniforme nos joints (mesma aproximação do three.js).
        n = (m * vec4<f32>(n, 0.0)).xyz;
        t = (m * vec4<f32>(t, 0.0)).xyz;
    }

    let nn = normalize(select(vec3<f32>(0.0, 1.0, 0.0), n, dot(n, n) > 1e-12));
    var out_t = vec3<f32>(0.0);
    if v.tangent.w != 0.0 {
        // Gram-Schmidt contra a normal deformada (morph não altera tangentes aqui).
        let ortho = t - nn * dot(nn, t);
        out_t = select(t, normalize(ortho), dot(ortho, ortho) > 1e-12);
    }
    dst[job.out_first + i] = Vtx(vec4<f32>(p, 1.0), vec4<f32>(nn, 0.0), vec4<f32>(out_t, v.tangent.w));
}
