//! PS-17: the export's analysis annotations. They sit beside the definition
//! sections, never inside them, so no TIR digest, obligation checksum or
//! signature fingerprint covers them (PS-19); their own digest does.
use crate::wire::Cx;
use crate::{Export, ImportError, Location, refuse};
use hirsel_codec::{Snapshot, Writer, snapshot_enum, snapshot_struct};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::SignatureKind;

/// PS-17: the digest over the whole annotations section.
pub const ANNOTATION_DIGEST_DOMAIN: &str = "hirsel.analysis.annotations.v1";

/// Where an annotation attaches within its definition, by declaration order:
/// a function's parameters (`Parameter i` is the TIR entry body's
/// `parameters[i]`), a record's fields (a distinct type's field is field 0), a
/// variant's payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Anchor {
    Item,
    Parameter(u32),
    Field(u32),
    Payload { variant: u32, field: u32 },
}
snapshot_enum!(Anchor {
    Item,
    Parameter(index),
    Field(index),
    Payload { variant, field }
} in Cx);

/// One `@analysis(name: payload)`: the payload is its token spellings, in
/// order, without trivia.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Annotation {
    pub path: DefPath,
    pub anchor: Anchor,
    pub name: String,
    pub payload: Vec<String>,
    pub location: Location,
}
snapshot_struct!(Annotation {
    path,
    anchor,
    name,
    payload,
    location
} in Cx);

/// `H("hirsel.analysis.annotations.v1", section text)`.
pub fn annotation_digest(annotations: &[Annotation]) -> ContentHash {
    let mut w = Writer::with_context(Cx::new(std::rc::Rc::new(|_| Location::unknown())));
    annotations.to_vec().write(&mut w);
    ContentHash::of_parts(ANNOTATION_DIGEST_DOMAIN, [w.finish().0.as_bytes()])
}

/// A dotted analysis name: nonempty segments of letters, digits and `_`.
fn valid_name(name: &str) -> bool {
    name.split('.').all(|segment| {
        segment
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && segment.chars().all(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Import's checks: sorted by definition, each on an exported signature at a
/// site of its kind, and the digest recomputes.
pub(crate) fn verify(export: &Export) -> Result<(), ImportError> {
    let mut previous: Option<&DefPath> = None;
    for (index, annotation) in export.annotations.iter().enumerate() {
        let at = format!("annotations[{index}]");
        if previous.is_some_and(|p| p > &annotation.path) {
            return Err(refuse(at, "annotations are not sorted by definition"));
        }
        previous = Some(&annotation.path);
        if !valid_name(&annotation.name) {
            return Err(refuse(
                at,
                format!("{:?} is not a dotted analysis name", annotation.name),
            ));
        }
        if annotation.payload.iter().any(String::is_empty) {
            return Err(refuse(at, "a payload token is empty"));
        }
        let Some(section) = export.signatures.iter().find(|s| s.path == annotation.path) else {
            return Err(refuse(
                at,
                format!("{} has no exported signature", annotation.path),
            ));
        };
        let fits = match (&section.public.kind, annotation.anchor) {
            (_, Anchor::Item) => true,
            (SignatureKind::Function(f), Anchor::Parameter(i)) => (i as usize) < f.parameters.len(),
            (SignatureKind::Type(_), Anchor::Field(_) | Anchor::Payload { .. }) => true,
            _ => false,
        };
        if !fits {
            return Err(refuse(
                at,
                format!(
                    "{:?} is not a site of {}",
                    annotation.anchor, annotation.path
                ),
            ));
        }
    }
    if annotation_digest(&export.annotations) != export.annotation_digest {
        return Err(refuse(
            "annotation_digest",
            "does not match the annotations",
        ));
    }
    Ok(())
}
