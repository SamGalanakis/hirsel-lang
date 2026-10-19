//! Definition content hashes (LANG-6): a name-independent code identity per
//! definition. The encoding is the PS-8 wire text under [`Cx`] in content
//! mode: every definition path is written as its hash, a reference into the
//! definition's own recursive group as a group marker, a signature's nominal
//! roles by role name, and spans, local and parameter names and name-derived
//! fingerprints are erased.
use crate::wire::Cx;
use crate::{Location, collect};
use hirsel_codec::{DecodeError, Next, Reader, Snapshot, Writer};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::TIR_VERSION;
use hirsel_tir::{
    BlockId, BodyId, Callee, Definition, DefinitionRef, Rvalue, RvalueId, Terminator,
};
use hirsel_types::SignatureRef;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// The domain of a definition's hash.
pub const DEFINITION_HASH_DOMAIN: &str = "hirsel.definition.hash.v1";
const CONTENT_DOMAIN: &str = "hirsel.definition.content.v1";
const ROUND_DOMAIN: &str = "hirsel.definition.round.v1";
const GROUP_DOMAIN: &str = "hirsel.definition.group.v1";

/// What one definition's hash covers.
#[derive(Clone, Debug)]
pub enum DefinitionContent {
    /// A definition the program implements: its signature (a type's owner
    /// view) and its TIR, when it has a body. Its own name is excluded.
    Body {
        signature: SignatureRef,
        tir: Option<DefinitionRef>,
    },
    /// A definition whose meaning is attached to its name or its source text:
    /// a registry operation, an extern, a signature fragment or a generic
    /// generated declaration. Its full path, its signature, its declaration's
    /// semantic source fields and the definitions it reads.
    Named {
        signature: SignatureRef,
        source: Vec<Vec<u8>>,
        reads: Vec<DefPath>,
    },
}

/// Content-mode naming state carried by [`Cx`].
#[derive(Default)]
pub(crate) struct Naming {
    /// Hashes of the definitions outside the group being encoded.
    hashes: BTreeMap<DefPath, ContentHash>,
    /// The group being encoded, each member with its index.
    group: BTreeMap<DefPath, usize>,
    /// Emit pass: each reference to a group member, in encoding order.
    members: Vec<usize>,
    /// Collect pass: every definition named, first occurrence first.
    named: Vec<DefPath>,
    seen: BTreeSet<DefPath>,
    /// Emit pass: the first definition named without a known hash.
    missing: Option<DefPath>,
}

impl Naming {
    pub(crate) fn reference(&mut self, path: &DefPath, collecting: bool) -> String {
        if collecting && self.seen.insert(path.clone()) {
            self.named.push(path.clone());
        }
        if let Some(&member) = self.group.get(path) {
            if !collecting {
                self.members.push(member);
            }
            return "group".into();
        }
        match self.hashes.get(path) {
            Some(hash) => hex(hash),
            None => {
                if !collecting && self.missing.is_none() {
                    self.missing = Some(path.clone());
                }
                "?".into()
            }
        }
    }
}

fn hex(hash: &ContentHash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_content(content: &DefinitionContent, w: &mut Writer<Cx>) {
    match content {
        DefinitionContent::Body { signature, tir } => {
            w.open("Body");
            signature.write(w);
            match tir {
                Some(definition) => definition.write(w),
                None => {
                    w.open("None");
                    w.close();
                }
            }
        }
        DefinitionContent::Named {
            signature,
            source,
            reads,
        } => {
            w.open("Named");
            w.text(&signature.definition.to_string());
            signature.write(w);
            w.open("list");
            for field in source {
                w.bytes(field);
            }
            w.close();
            reads.write(w);
        }
    }
    w.close();
}

fn encode(content: &DefinitionContent, naming: Naming) -> (String, Naming) {
    let mut w = Writer::with_context(Cx::new(Rc::new(|_| Location::unknown())));
    w.context.content = Some(naming);
    collect(&mut w, |w| write_content(content, w));
    write_content(content, &mut w);
    let (text, cx) = w.finish();
    (text, cx.content.expect("content naming"))
}

/// Every definition `content` names, first occurrence first.
pub fn references(content: &DefinitionContent) -> Vec<DefPath> {
    encode(content, Naming::default()).1.named
}

/// One recursive group's hash preimage (LANG-6): what a code store keeps
/// for each member, from which every member's hash is recomputed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionGroup {
    /// The members, ordered by hash and then text, so the order depends on
    /// no name.
    pub members: Vec<DefPath>,
    /// Each member's content text.
    pub texts: Vec<String>,
    /// For each member, the member each group marker of its text names, in
    /// text order.
    pub uses: Vec<Vec<usize>>,
    /// Each member's definition hash.
    pub hashes: Vec<ContentHash>,
}

/// The definition hashes of a recursive group's members from their content
/// texts and group references. Each member's color is its text's hash; a
/// round rehashes each color with the colors of the members it references
/// until the partition into equal colors is stable, and each member's hash
/// binds the group's sorted colors and its own. An error names a reference
/// outside the group.
pub fn group_hashes(texts: &[String], uses: &[Vec<usize>]) -> Result<Vec<ContentHash>, String> {
    if texts.len() != uses.len() {
        return Err(format!(
            "{} texts with {} reference lists",
            texts.len(),
            uses.len()
        ));
    }
    if let Some(&outside) = uses.iter().flatten().find(|&&j| j >= texts.len()) {
        return Err(format!(
            "group reference {outside} outside a group of {}",
            texts.len()
        ));
    }
    let mut colors: Vec<ContentHash> = texts
        .iter()
        .map(|text| {
            ContentHash::of_parts(
                CONTENT_DOMAIN,
                [TIR_VERSION.to_le_bytes().as_slice(), text.as_bytes()],
            )
        })
        .collect();
    let distinct = |colors: &[ContentHash]| colors.iter().collect::<BTreeSet<_>>().len();
    loop {
        let refined: Vec<ContentHash> = uses
            .iter()
            .enumerate()
            .map(|(i, used)| {
                let parts = std::iter::once(colors[i].0.as_slice())
                    .chain(used.iter().map(|&j| colors[j].0.as_slice()));
                ContentHash::of_parts(ROUND_DOMAIN, parts)
            })
            .collect();
        let stable = distinct(&refined) == distinct(&colors);
        colors = refined;
        if stable {
            break;
        }
    }
    let mut sorted = colors.clone();
    sorted.sort();
    let group_hash = ContentHash::of_parts(GROUP_DOMAIN, sorted.iter().map(|c| c.0.as_slice()));
    Ok(colors
        .iter()
        .map(|color| {
            ContentHash::of_parts(
                DEFINITION_HASH_DOMAIN,
                [group_hash.0.as_slice(), color.0.as_slice()],
            )
        })
        .collect())
}

/// The recursive groups (strongly connected components) of `contents`,
/// dependencies first, with each member's content text, group references
/// and hash. `contents` must hold every definition they name. A reference
/// into the member's own group is written as a group marker; no name or
/// iteration order enters a hash.
pub fn definition_groups(
    contents: &BTreeMap<DefPath, DefinitionContent>,
) -> Result<Vec<DefinitionGroup>, String> {
    let paths: Vec<&DefPath> = contents.keys().collect();
    let index: BTreeMap<&DefPath, usize> = paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();
    let mut edges = Vec::with_capacity(paths.len());
    for (path, content) in contents {
        let mut targets = vec![];
        for reference in references(content) {
            match index.get(&reference) {
                Some(&target) => targets.push(target),
                None => return Err(format!("{path} names {reference}, which has no content")),
            }
        }
        edges.push(targets);
    }
    let mut hashes = BTreeMap::new();
    let mut groups = vec![];
    for component in components(&edges) {
        let members: BTreeMap<DefPath, usize> = component
            .iter()
            .enumerate()
            .map(|(i, &node)| (paths[node].clone(), i))
            .collect();
        let mut texts = Vec::with_capacity(component.len());
        let mut uses = Vec::with_capacity(component.len());
        let group_paths: Vec<DefPath> = component.iter().map(|&node| paths[node].clone()).collect();
        for path in &group_paths {
            let naming = Naming {
                hashes: std::mem::take(&mut hashes),
                group: members.clone(),
                ..Naming::default()
            };
            let (text, naming) = encode(&contents[path], naming);
            if let Some(missing) = naming.missing {
                return Err(format!("{path} names {missing} before its hash"));
            }
            hashes = naming.hashes;
            texts.push(text);
            uses.push(naming.members);
        }
        let member_hashes = group_hashes(&texts, &uses)?;
        // Canonical member order: by hash, then text.
        let mut order: Vec<usize> = (0..texts.len()).collect();
        order.sort_by(|&a, &b| (member_hashes[a], &texts[a]).cmp(&(member_hashes[b], &texts[b])));
        let mut rank = vec![0; order.len()];
        for (position, &member) in order.iter().enumerate() {
            rank[member] = position;
        }
        for (path, hash) in group_paths.iter().zip(&member_hashes) {
            hashes.insert(path.clone(), *hash);
        }
        groups.push(DefinitionGroup {
            members: order.iter().map(|&m| group_paths[m].clone()).collect(),
            texts: order
                .iter()
                .map(|&m| std::mem::take(&mut texts[m]))
                .collect(),
            uses: order
                .iter()
                .map(|&m| uses[m].iter().map(|&j| rank[j]).collect())
                .collect(),
            hashes: order.iter().map(|&m| member_hashes[m]).collect(),
        });
    }
    Ok(groups)
}

/// The hash of every definition in `contents`, which must hold every
/// definition they name ([`definition_groups`]).
pub fn definition_hashes(
    contents: &BTreeMap<DefPath, DefinitionContent>,
) -> Result<BTreeMap<DefPath, ContentHash>, String> {
    Ok(definition_groups(contents)?
        .into_iter()
        .flat_map(|group| group.members.into_iter().zip(group.hashes))
        .collect())
}

/// A reference written in a content text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentReference {
    /// A definition outside the group, by its hash.
    Definition(ContentHash),
    /// A member of the text's own recursive group.
    Group,
}

/// Every reference in a content text, in text order. The text must be one
/// well-formed value whose references are definition hashes or group
/// markers.
pub fn content_references(text: &str) -> Result<Vec<ContentReference>, DecodeError> {
    let mut r = Reader::new(text);
    let mut references = vec![];
    let mut depth = 0usize;
    loop {
        match r.next_kind() {
            Next::Open => {
                if r.open_any()? == "Ref" {
                    let reference = r.text()?;
                    references.push(if reference == "group" {
                        ContentReference::Group
                    } else {
                        ContentReference::Definition(
                            parse_hex(&reference)
                                .ok_or_else(|| r.error(format!("bad reference {reference:?}")))?,
                        )
                    });
                    r.close()?;
                } else {
                    depth += 1;
                }
            }
            Next::Close if depth > 0 => {
                r.close()?;
                depth -= 1;
            }
            Next::Close => return Err(r.error("unbalanced ')'")),
            Next::Text => drop(r.text()?),
            Next::Bytes => drop(r.bytes()?),
            Next::Integer => drop(r.int()?),
            Next::End => return Err(r.error("unexpected end of text")),
        }
        if depth == 0 {
            break;
        }
    }
    r.finish()?;
    Ok(references)
}

fn parse_hex(text: &str) -> Option<ContentHash> {
    if text.len() != 64 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut hash = [0u8; 32];
    for (byte, pair) in hash.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(ContentHash(hash))
}

/// Tarjan's strongly connected components, dependencies before dependents.
fn components(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNSEEN: usize = usize::MAX;
    let mut order = vec![UNSEEN; edges.len()];
    let mut low = vec![0; edges.len()];
    let mut on_stack = vec![false; edges.len()];
    let mut stack = vec![];
    let mut components = vec![];
    let mut next = 0;
    for root in 0..edges.len() {
        if order[root] != UNSEEN {
            continue;
        }
        // (node, next edge to visit)
        let mut frames = vec![(root, 0)];
        order[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(node, edge)) = frames.last() {
            if let Some(&target) = edges[node].get(edge) {
                frames.last_mut().expect("frame").1 += 1;
                if order[target] == UNSEEN {
                    order[target] = next;
                    low[target] = next;
                    next += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    frames.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(order[target]);
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == order[node] {
                let mut component = vec![];
                loop {
                    let member = stack.pop().expect("component member");
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    components
}

/// Where a definition's TIR names another definition it may run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallSiteKind {
    /// The block whose terminator calls the definition.
    Call(BlockId),
    /// The rvalue that makes a function value of the definition.
    Reference(RvalueId),
}

/// One call site with the callee's definition hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallSite {
    pub body: BodyId,
    pub kind: CallSiteKind,
    pub callee: DefPath,
    pub hash: ContentHash,
}

/// Every call and function reference in `definition`, in body and block
/// order, with its callee's hash from `hashes`; an error names a callee
/// `hashes` lacks.
pub fn call_sites(
    definition: &Definition,
    hashes: &BTreeMap<DefPath, ContentHash>,
) -> Result<Vec<CallSite>, DefPath> {
    let mut sites = vec![];
    for (b, body) in definition.bodies.iter().enumerate() {
        let body_id = BodyId(b as u32);
        let mut site = |kind, callee: &DefPath| {
            let hash = *hashes.get(callee).ok_or_else(|| callee.clone())?;
            sites.push(CallSite {
                body: body_id,
                kind,
                callee: callee.clone(),
                hash,
            });
            Ok::<_, DefPath>(())
        };
        for (i, block) in body.blocks.iter().enumerate() {
            if let Terminator::Call {
                callee: Callee::Definition { path, .. },
                ..
            } = &block.terminator
            {
                site(CallSiteKind::Call(BlockId(i as u32)), path)?;
            }
        }
        for (i, rvalue) in body.rvalues.iter().enumerate() {
            if let Rvalue::FunctionReference { definition, .. } = rvalue {
                site(CallSiteKind::Reference(RvalueId(i as u32)), definition)?;
            }
        }
    }
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_references_read_ref_nodes_only_and_refuse_malformed_text() {
        let a = ContentHash([0xab; 32]);
        let text = format!(
            "(Body (Ref \"{}\") \"(Ref \\\"group\\\")\" #00 -1 (Ref \"group\"))",
            hex(&a)
        );
        assert_eq!(
            content_references(&text).unwrap(),
            [ContentReference::Definition(a), ContentReference::Group]
        );
        for bad in [
            format!("(Body (Ref \"{}\"))", hex(&a).to_uppercase()),
            format!("(Body (Ref \"+{}\"))", &hex(&a)[1..]),
            "(Body (Ref \"?\"))".into(),
            "(Body".into(),
            String::new(),
            "(Body))".into(),
            "(A) (B)".into(),
        ] {
            assert!(content_references(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn group_hashes_refuse_references_outside_the_group() {
        let texts = vec!["(A)".to_string(), "(B)".to_string()];
        assert!(group_hashes(&texts, &[vec![1], vec![0]]).is_ok());
        assert!(group_hashes(&texts, &[vec![2], vec![]]).is_err());
        assert!(group_hashes(&texts, &[vec![]]).is_err());
        // A member's hash depends on whom it references, not on positions.
        let swapped = vec!["(B)".to_string(), "(A)".to_string()];
        let mut forward = group_hashes(&texts, &[vec![1], vec![]]).unwrap();
        let mut backward = group_hashes(&swapped, &[vec![], vec![0]]).unwrap();
        backward.reverse();
        assert_eq!(forward, backward);
        forward.dedup();
        assert_eq!(forward.len(), 2);
    }
}
