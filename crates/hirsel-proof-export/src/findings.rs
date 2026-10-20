//! PS-18: the findings sidecar an out-of-process analysis writes. A finding
//! is a located report, keyed by the export digest it was computed from; it
//! has no obligation id or checksum and no compiler path reads it, so it
//! never rejects a program or licenses a check removal (PS-19).
use crate::{Export, Location};
use hirsel_syntax::ContentHash;
use serde_json::{Map, Value, json};

/// The sidecar format.
pub const FINDINGS_FORMAT: &str = "hirsel.analysis.findings.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}
impl Severity {
    pub const ALL: [Self; 3] = [Self::Error, Self::Warning, Self::Note];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
        }
    }
}

/// A secondary site with its own message, such as where a secret entered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Related {
    pub location: Location,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    /// The analysis name, as its annotations spell it.
    pub analysis: String,
    /// The analysis's own code for the finding, such as `TAINT-1`.
    pub code: String,
    pub severity: Severity,
    /// The written `DefPath` of the definition the finding is in.
    pub definition: String,
    pub location: Location,
    pub message: String,
    pub related: Vec<Related>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Findings {
    /// The digest of the export the analysis read.
    pub export: ContentHash,
    /// `tool@version`.
    pub analyzer: String,
    pub findings: Vec<Finding>,
}

impl Findings {
    /// Parse a sidecar; unknown or missing fields refuse it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let root = object(
            &root,
            "the sidecar",
            &["format", "export", "analyzer", "findings"],
        )?;
        if root["format"].as_str() != Some(FINDINGS_FORMAT) {
            return Err(format!("format is not {FINDINGS_FORMAT:?}"));
        }
        let analyzer = string(&root["analyzer"], "analyzer")?;
        if !analyzer
            .rsplit_once('@')
            .is_some_and(|(tool, version)| !tool.is_empty() && !version.is_empty())
        {
            return Err("analyzer is not tool@version".into());
        }
        let findings = root["findings"]
            .as_array()
            .ok_or("findings is not an array")?
            .iter()
            .enumerate()
            .map(|(i, f)| finding(f, &format!("findings[{i}]")))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            export: hash(&string(&root["export"], "export")?).ok_or("export is not a hash")?,
            analyzer,
            findings,
        })
    }

    /// The canonical text: keys sorted, two-space indentation, a final newline.
    pub fn to_json(&self) -> String {
        let value = json!({
            "format": FINDINGS_FORMAT,
            "export": spell(&self.export),
            "analyzer": self.analyzer,
            "findings": self.findings.iter().map(|f| json!({
                "analysis": f.analysis,
                "code": f.code,
                "severity": f.severity.as_str(),
                "definition": f.definition,
                "location": location_json(&f.location),
                "message": f.message,
                "related": f.related.iter().map(|r| json!({
                    "location": location_json(&r.location),
                    "message": r.message,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        serde_json::to_string_pretty(&value).expect("JSON values encode") + "\n"
    }

    /// Whether these findings are current for `export` and name only its
    /// definitions.
    pub fn check(&self, export: &Export) -> Result<(), String> {
        let digest = export.digest();
        if self.export != digest {
            return Err(format!(
                "recorded for export {}, not {}",
                spell(&self.export),
                spell(&digest)
            ));
        }
        for (i, f) in self.findings.iter().enumerate() {
            if !export
                .signatures
                .iter()
                .any(|s| s.path.to_string() == f.definition)
            {
                return Err(format!(
                    "findings[{i}].definition {} is not in the export",
                    f.definition
                ));
            }
        }
        Ok(())
    }
}

fn finding(value: &Value, at: &str) -> Result<Finding, String> {
    let f = object(
        value,
        at,
        &[
            "analysis",
            "code",
            "severity",
            "definition",
            "location",
            "message",
            "related",
        ],
    )?;
    let field = |name: &str| string(&f[name], &format!("{at}.{name}"));
    let severity = field("severity")?;
    let analysis = field("analysis")?;
    if analysis.is_empty() {
        return Err(format!("{at}.analysis is empty"));
    }
    Ok(Finding {
        analysis,
        code: field("code")?,
        severity: Severity::ALL
            .into_iter()
            .find(|s| s.as_str() == severity)
            .ok_or(format!("{at}.severity {severity:?} is not a severity"))?,
        definition: field("definition")?,
        location: location(&f["location"], &format!("{at}.location"))?,
        message: field("message")?,
        related: f["related"]
            .as_array()
            .ok_or(format!("{at}.related is not an array"))?
            .iter()
            .enumerate()
            .map(|(j, r)| {
                let at = format!("{at}.related[{j}]");
                let r = object(r, &at, &["location", "message"])?;
                Ok(Related {
                    location: location(&r["location"], &format!("{at}.location"))?,
                    message: string(&r["message"], &format!("{at}.message"))?,
                })
            })
            .collect::<Result<_, String>>()?,
    })
}

fn location(value: &Value, at: &str) -> Result<Location, String> {
    let l = object(
        value,
        at,
        &["module", "line", "column", "end_line", "end_column"],
    )?;
    let number = |name: &str| {
        l[name]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(format!("{at}.{name} is not a u32"))
    };
    Ok(Location {
        module: string(&l["module"], &format!("{at}.module"))?,
        line: number("line")?,
        column: number("column")?,
        end_line: number("end_line")?,
        end_column: number("end_column")?,
    })
}

fn location_json(l: &Location) -> Value {
    json!({
        "module": l.module,
        "line": l.line,
        "column": l.column,
        "end_line": l.end_line,
        "end_column": l.end_column,
    })
}

fn object<'v>(
    value: &'v Value,
    at: &str,
    fields: &[&str],
) -> Result<&'v Map<String, Value>, String> {
    let map = value.as_object().ok_or(format!("{at} is not an object"))?;
    if let Some(field) = map.keys().find(|k| !fields.contains(&k.as_str())) {
        return Err(format!("{at} has an unknown field {field:?}"));
    }
    if let Some(field) = fields.iter().find(|f| !map.contains_key(**f)) {
        return Err(format!("{at} lacks {field:?}"));
    }
    Ok(map)
}

fn string(value: &Value, at: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or(format!("{at} is not a string"))
}

/// `#` and 64 lowercase hex digits, the export's spelling.
pub fn spell(hash: &ContentHash) -> String {
    let mut text = String::from("#");
    for byte in hash.0 {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn hash(text: &str) -> Option<ContentHash> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 64 || hex.bytes().any(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(ContentHash(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32) -> Location {
        Location {
            module: "standalone@0".into(),
            line,
            column: 3,
            end_line: line,
            end_column: 9,
        }
    }

    fn sample() -> Findings {
        Findings {
            export: ContentHash([7; 32]),
            analyzer: "taint@0.1".into(),
            findings: vec![Finding {
                analysis: "taint".into(),
                code: "TAINT-1".into(),
                severity: Severity::Error,
                definition: "standalone@0/show".into(),
                location: at(2),
                message: "a secret reaches stdout".into(),
                related: vec![Related {
                    location: at(1),
                    message: "the secret parameter".into(),
                }],
            }],
        }
    }

    #[test]
    fn findings_round_trip_through_their_canonical_text() {
        let text = sample().to_json();
        assert_eq!(Findings::parse(&text), Ok(sample()));
        assert_eq!(Findings::parse(&text).unwrap().to_json(), text);
        assert!(text.contains("\"export\": \"#0707"), "{text}");
        let empty = Findings {
            findings: vec![],
            ..sample()
        };
        assert_eq!(Findings::parse(&empty.to_json()), Ok(empty));
    }

    #[test]
    fn a_malformed_sidecar_is_refused_with_its_field() {
        let edited = |edit: &dyn Fn(&mut Value)| {
            let mut value: Value = serde_json::from_str(&sample().to_json()).unwrap();
            edit(&mut value);
            Findings::parse(&value.to_string()).unwrap_err()
        };
        type Edit = dyn Fn(&mut Value);
        let cases: [(&Edit, &str); 7] = [
            (
                &|v| v["format"] = json!("hirsel.proof.sidecar.v1"),
                "format is not",
            ),
            (
                &|v| v["findings"][0]["severity"] = json!("fatal"),
                "findings[0].severity \"fatal\" is not a severity",
            ),
            (
                &|v| v["findings"][0]["obligation"] = json!("x"),
                "findings[0] has an unknown field \"obligation\"",
            ),
            (
                &|v| {
                    v["findings"][0].as_object_mut().unwrap().remove("message");
                },
                "findings[0] lacks \"message\"",
            ),
            (
                &|v| v["analyzer"] = json!("taint"),
                "analyzer is not tool@version",
            ),
            (&|v| v["export"] = json!("#07"), "export is not a hash"),
            (
                &|v| v["findings"][0]["related"][0]["location"]["line"] = json!(-1),
                "findings[0].related[0].location.line is not a u32",
            ),
        ];
        for (edit, message) in cases {
            let error = edited(edit);
            assert!(error.contains(message), "{error} lacks {message}");
        }
    }
}
