//! Build-time core declarations use ordinary product records and type refs.
use hirsel_artifact_store::Reference;
use hirsel_product_codec::BaseIndex;
use hirsel_syntax::{ContentHash, DefId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize)]
pub struct CoreProducts {
    pub revision: ContentHash,
    pub base: BaseIndex,
    pub signatures: Vec<(DefId, Reference)>,
    pub variants: BTreeSet<String>,
}
