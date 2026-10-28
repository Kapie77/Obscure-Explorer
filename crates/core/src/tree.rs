use std::collections::BTreeMap;

/// Árvore de diretórios construída a partir dos caminhos das entradas.
#[derive(Default, Debug)]
pub struct DirNode {
    pub dirs: BTreeMap<String, DirNode>,
    /// nome do arquivo -> índice da entrada no `Archive`
    pub files: BTreeMap<String, usize>,
}

impl DirNode {
    /// Desce pela árvore seguindo `path`; `None` se alguma pasta não existir.
    pub fn get(&self, path: &[String]) -> Option<&DirNode> {
        let mut cur = self;
        for part in path {
            cur = cur.dirs.get(part)?;
        }
        Some(cur)
    }

    pub fn build<'a>(items: impl Iterator<Item = (usize, &'a str)>) -> DirNode {
        let mut root = DirNode::default();
        for (index, path) in items {
            let mut parts = path.split('/').filter(|s| !s.is_empty()).peekable();
            let mut cur = &mut root;
            while let Some(part) = parts.next() {
                if parts.peek().is_some() {
                    cur = cur.dirs.entry(part.to_string()).or_default();
                } else {
                    cur.files.insert(part.to_string(), index);
                }
            }
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_tree() {
        let paths = ["a/b/c.dic", "a/d.hvt", "e.txt"];
        let tree = DirNode::build(paths.iter().enumerate().map(|(i, p)| (i, *p)));
        assert_eq!(tree.files.get("e.txt"), Some(&2));
        let a = tree.dirs.get("a").unwrap();
        assert_eq!(a.files.get("d.hvt"), Some(&1));
        assert_eq!(a.dirs.get("b").unwrap().files.get("c.dic"), Some(&0));
    }
}
