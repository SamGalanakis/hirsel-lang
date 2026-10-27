//! PS-13 facts from the independent reference/discharged import comparison.
//! A definition's complete check inventory can span several native bodies.
use hirsel_lir::discharge::{Discharge, Discharged, Form};
use hirsel_syntax::DefPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum SiteForm {
    Check(String),
    Integer,
    Negation,
    Projection { place: u32, rule: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Site {
    body: u32,
    block: u32,
    statement: u32,
    form: SiteForm,
}
impl Site {
    fn from_site(site: &Discharged) -> Self {
        Self {
            body: site.body,
            block: site.block,
            statement: site.statement,
            form: match &site.form {
                Form::Check(rule) => SiteForm::Check(rule.clone()),
                Form::Integer => SiteForm::Integer,
                Form::Negation => SiteForm::Negation,
                Form::Projection { place, rule } => SiteForm::Projection {
                    place: *place,
                    rule: rule.clone(),
                },
            },
        }
    }
    fn site(&self) -> Discharged {
        Discharged {
            body: self.body,
            block: self.block,
            statement: self.statement,
            form: match &self.form {
                SiteForm::Check(rule) => Form::Check(rule.clone()),
                SiteForm::Integer => Form::Integer,
                SiteForm::Negation => Form::Negation,
                SiteForm::Projection { place, rule } => Form::Projection {
                    place: *place,
                    rule: rule.clone(),
                },
            },
        }
    }
}

/// Ordinary metadata bound to the Instance's C-owned descriptor. It grants no
/// source or native authority on its own; only actual miss validation creates it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeDischargeFacts {
    sites: Vec<Site>,
}
impl NativeDischargeFacts {
    pub(super) fn retained_metadata_bytes(&self) -> u64 {
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.vec(&self.sites, |w, site| match &site.form {
            SiteForm::Check(rule) | SiteForm::Projection { rule, .. } => w.add(rule.capacity()),
            SiteForm::Integer | SiteForm::Negation => (),
        });
        weight.bytes()
    }
    pub(super) fn checked(sites: &BTreeSet<Discharged>) -> Self {
        Self {
            sites: sites.iter().map(Site::from_site).collect(),
        }
    }
    pub(super) fn canonical(&self) -> bool {
        self.sites
            .windows(2)
            .all(|pair| pair[0].site() < pair[1].site())
    }
    pub fn removed(&self) -> BTreeSet<Discharged> {
        self.sites.iter().map(Site::site).collect()
    }
}

/// Preserve the batch validator's union across every selected body of a source
/// definition, then require every licensed site of a compiled definition. Call
/// this only after discovery has reached its complete instance fixed point.
pub(super) fn complete<'a>(
    plan: &Discharge,
    instances: impl IntoIterator<Item = (&'a DefPath, &'a NativeDischargeFacts)>,
) -> Result<Discharge, String> {
    let mut removed = Discharge::default();
    for (path, facts) in instances {
        if !facts.canonical() {
            return Err("noncanonical native discharge facts".into());
        }
        let here = facts.removed();
        let Some(expected) = plan.sites(path) else {
            if !here.is_empty() {
                return Err(format!("native discharge has no licensed plan for {path}"));
            }
            continue;
        };
        if !here.is_subset(expected) {
            return Err(format!(
                "native discharge exceeds its licensed plan for {path}"
            ));
        }
        removed.0.entry(path.clone()).or_default().extend(here);
    }
    for (path, actual) in &removed.0 {
        if let Some(missing) = plan.0[path].difference(actual).next() {
            return Err(format!(
                "accepted discharge has no native check in {path}: body {} block {} statement {}",
                missing.body, missing.block, missing.statement,
            ));
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_discharge_unions_bodies_and_refuses_missing_or_unlicensed_sites() {
        let path = hirsel_engine::entry_path(&hirsel_engine::default_context());
        let site = |body| Discharged {
            body,
            block: 0,
            statement: 1,
            form: Form::Integer,
        };
        let first = NativeDischargeFacts::checked(&BTreeSet::from([site(0)]));
        let second = NativeDischargeFacts::checked(&BTreeSet::from([site(1)]));
        let plan = Discharge(std::collections::BTreeMap::from([(
            path.clone(),
            BTreeSet::from([site(0), site(1)]),
        )]));
        assert_eq!(
            complete(&plan, [(&path, &first), (&path, &second)]).unwrap(),
            plan
        );
        assert!(
            complete(&plan, [(&path, &first)])
                .unwrap_err()
                .contains("no native check")
        );
        assert!(complete(&plan, [(&path, &NativeDischargeFacts::default())]).is_err());
        let unlicensed = NativeDischargeFacts::checked(&BTreeSet::from([site(2)]));
        assert!(
            complete(&plan, [(&path, &unlicensed)])
                .unwrap_err()
                .contains("exceeds")
        );
        assert!(
            complete(&Discharge::default(), [(&path, &first)])
                .unwrap_err()
                .contains("no licensed plan")
        );
        let mut duplicate = first.clone();
        duplicate.sites.push(duplicate.sites[0].clone());
        assert!(
            complete(&plan, [(&path, &duplicate)])
                .unwrap_err()
                .contains("noncanonical")
        );
        let mut reversed = NativeDischargeFacts::checked(&plan.0[&path]);
        reversed.sites.reverse();
        assert!(complete(&plan, [(&path, &reversed)]).is_err());
        // A definition with no selected native body is kept by the report.
        assert!(complete(&plan, []).unwrap().0.is_empty());
    }
}
