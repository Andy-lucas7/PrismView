//! Grafo de objetos da cena 3D por `parentId`: ordem topológica (pai antes do filho), quebra de
//! ciclos/pais inexistentes (viram raízes + aviso), visibilidade herdada e matrizes de mundo.
//! Determinístico: a ordem segue a ordem de declaração dos objetos.

use std::collections::HashMap;

use glam::Mat4;

use super::config::Scene3dObject;

#[derive(Debug, Default)]
pub struct SceneGraph {
    /// Índice do pai (em `objects`) de cada objeto; `None` = raiz.
    pub parent: Vec<Option<usize>>,
    /// Índices em ordem de percurso (pais antes dos filhos, irmãos na ordem original).
    pub order: Vec<usize>,
    /// Problemas encontrados (pai inexistente, ciclo, id duplicado).
    pub warnings: Vec<String>,
}

impl SceneGraph {
    pub fn resolve(objects: &[Scene3dObject]) -> Self {
        let count = objects.len();
        let mut warnings = Vec::new();

        let mut by_id: HashMap<&str, usize> = HashMap::with_capacity(count);
        for (i, obj) in objects.iter().enumerate() {
            if obj.id.is_empty() {
                continue;
            }
            // O primeiro objeto com o id vence (referências existentes continuam válidas).
            if by_id.contains_key(obj.id.as_str()) {
                warnings.push(format!("id duplicado `{}`", obj.id));
            } else {
                by_id.insert(obj.id.as_str(), i);
            }
        }

        let mut parent: Vec<Option<usize>> = vec![None; count];
        for (i, obj) in objects.iter().enumerate() {
            let Some(pid) = obj.parent_id.as_deref().filter(|p| !p.is_empty()) else {
                continue;
            };
            match by_id.get(pid) {
                Some(&p) if p != i => parent[i] = Some(p),
                Some(_) => warnings.push(format!("`{}` não pode ser pai de si mesmo", obj.id)),
                None => warnings.push(format!("pai `{pid}` de `{}` não existe", obj.id)),
            }
        }

        // Ciclos: subir mais de `count` passos nunca alcança uma raiz → solta o objeto.
        for i in 0..count {
            let mut cursor = parent[i];
            let mut steps = 0usize;
            while let Some(p) = cursor {
                steps += 1;
                if steps > count {
                    warnings.push(format!("ciclo de parentId em `{}`", objects[i].id));
                    parent[i] = None;
                    break;
                }
                cursor = parent[p];
            }
        }

        // Filhos por pai (ordem original) e percurso em profundidade iterativo.
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
        let mut roots: Vec<usize> = Vec::new();
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => children[*p].push(i),
                None => roots.push(i),
            }
        }
        let mut order = Vec::with_capacity(count);
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            for &c in children[i].iter().rev() {
                stack.push(c);
            }
        }

        Self {
            parent,
            order,
            warnings,
        }
    }

    /// Matrizes de mundo (espaço da cena), na mesma indexação de `objects`.
    pub fn world_matrices(&self, objects: &[Scene3dObject]) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = vec![Mat4::IDENTITY; objects.len()];
        for &i in &self.order {
            let local = objects[i].local_matrix();
            world[i] = match self.parent.get(i).copied().flatten() {
                Some(p) => world[p] * local,
                None => local,
            };
        }
        world
    }

    /// Visibilidade efetiva: um objeto só aparece se ele e todos os ancestrais estão visíveis.
    pub fn effective_visibility(&self, objects: &[Scene3dObject]) -> Vec<bool> {
        let mut visible = vec![false; objects.len()];
        for &i in &self.order {
            let parent_ok = match self.parent.get(i).copied().flatten() {
                Some(p) => visible[p],
                None => true,
            };
            visible[i] = parent_ok && objects[i].visible;
        }
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn obj(id: &str, parent: Option<&str>, x: f32) -> Scene3dObject {
        Scene3dObject {
            id: id.to_owned(),
            parent_id: parent.map(str::to_owned),
            position: [x, 0.0, 0.0],
            ..Scene3dObject::default()
        }
    }

    #[test]
    fn parent_before_child_and_world_composes() {
        // Filho declarado antes do pai: a ordem de percurso ainda coloca o pai primeiro.
        let objects = vec![obj("child", Some("root"), 2.0), obj("root", None, 10.0)];
        let g = SceneGraph::resolve(&objects);
        assert_eq!(g.order, vec![1, 0]);
        let world = g.world_matrices(&objects);
        assert!((world[0].transform_point3(Vec3::ZERO) - Vec3::new(12.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn cycles_and_missing_parents_become_roots() {
        let objects = vec![
            obj("a", Some("b"), 0.0),
            obj("b", Some("a"), 0.0),
            obj("c", Some("nope"), 0.0),
        ];
        let g = SceneGraph::resolve(&objects);
        assert_eq!(g.order.len(), 3);
        assert!(!g.warnings.is_empty());
        assert!(g.parent[2].is_none());
    }

    #[test]
    fn hidden_parent_hides_children() {
        let mut objects = vec![obj("p", None, 0.0), obj("k", Some("p"), 0.0)];
        objects[0].visible = false;
        let g = SceneGraph::resolve(&objects);
        assert_eq!(g.effective_visibility(&objects), vec![false, false]);
    }
}
