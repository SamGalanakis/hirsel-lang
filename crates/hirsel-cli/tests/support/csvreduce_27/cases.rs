//! Expected values frozen independently of the Hirsel implementation.
//! Go encoding/csv ebe2feef reader_test.go:50-100 (BSD-3-Clause) supplies
//! multiline/CRLF/NoEOL shapes; markers §/¶ are removed, numeric cells added.
//! Rayon ee0a00bd iter/test.rs:78-128 (MIT/Apache-2.0) supplies 0..1023 + 1.
pub struct Case {
    pub id: &'static str,
    pub csv: String,
    pub grains: Vec<u64>,
    pub expected: &'static str,
    pub error: bool,
}
fn valid(id: &'static str, csv: &str, expected: &'static str) -> Case {
    Case {
        id,
        csv: csv.into(),
        grains: vec![1, 3],
        expected,
        error: false,
    }
}
fn invalid(id: &'static str, csv: String, grain: u64, expected: &'static str) -> Case {
    Case {
        id,
        csv,
        grains: vec![grain],
        expected,
        error: true,
    }
}
pub fn cases(part: usize) -> Vec<Case> {
    match part {
        0 => vec![
            valid(
                "header-only",
                "label,value\n",
                "{\"count\":0,\"sum\":0,\"min\":null,\"max\":null}\n",
            ),
            valid(
                "one-positive",
                "label,value\na,7\n",
                "{\"count\":1,\"sum\":7,\"min\":7,\"max\":7}\n",
            ),
            valid(
                "mixed-signed",
                "label,value\na,7\nb,-3\nc,0\nd,12\ne,-9\n",
                "{\"count\":5,\"sum\":7,\"min\":-9,\"max\":12}\n",
            ),
        ],
        1 => vec![
            valid(
                "quoted-multiline",
                "label,value\n\"bb\nb\",4\n\"a,a\",-1\n",
                "{\"count\":2,\"sum\":3,\"min\":-1,\"max\":4}\n",
            ),
            valid(
                "quoted-number",
                "label,value\na,\"23\"\n",
                "{\"count\":1,\"sum\":23,\"min\":23,\"max\":23}\n",
            ),
            valid(
                "crlf",
                "label,value\r\n\"two\r\nline\",-2\r\nb,5\r\n",
                "{\"count\":2,\"sum\":3,\"min\":-2,\"max\":5}\n",
            ),
            valid(
                "no-final-newline",
                "label,value\na,8",
                "{\"count\":1,\"sum\":8,\"min\":8,\"max\":8}\n",
            ),
        ],
        2 => vec![
            invalid(
                "invalid-integer",
                "label,value\na,no\nb,2\n".into(),
                1,
                "Integer",
            ),
            invalid(
                "wrong-columns",
                "label,value\na,1,extra\n".into(),
                1,
                "Columns",
            ),
            invalid(
                "cell-sum-bound",
                "label,value\na,1000000001\n".into(),
                1,
                "CellBound",
            ),
            invalid(
                "negative-cell-bound",
                "label,value\na,-1000000001\n".into(),
                1,
                "CellBound",
            ),
            invalid("grain-zero", "label,value\n".into(), 0, "Grain"),
            invalid("grain-large", "label,value\n".into(), 4097, "Grain"),
            invalid(
                "chunk-cap",
                format!("label,value\n{}", "a,1\n".repeat(17)),
                1,
                "Grain",
            ),
            invalid(
                "column-cap",
                format!("{}\n", vec!["x"; 33].join(",")),
                1,
                "Columns",
            ),
            invalid("no-header", String::new(), 1, "Header"),
        ],
        3 => vec![invalid(
            "record-count-bound",
            format!("label,value\n{}", "a,1\n".repeat(4097)),
            256,
            "Records",
        )],
        4 => {
            let skew = (1..=16)
                .map(|i| format!("row,{i:0width$}\n", width = if i % 2 == 0 { 24 } else { 1 }))
                .collect::<String>();
            vec![
                valid(
                    "non-divisible-tail",
                    "label,value\na,1\nb,2\nc,3\nd,4\ne,5\n",
                    "{\"count\":5,\"sum\":15,\"min\":1,\"max\":5}\n",
                ),
                valid(
                    "sixteen-skewed-chunks",
                    &format!("label,value\n{skew}"),
                    "{\"count\":16,\"sum\":136,\"min\":1,\"max\":16}\n",
                ),
                valid(
                    "exact-cell-bounds",
                    "label,value\na,1000000000\nb,-1000000000\n",
                    "{\"count\":2,\"sum\":0,\"min\":-1000000000,\"max\":1000000000}\n",
                ),
            ]
        }
        5 => vec![Case {
            id: "rayon-map-reduce-1024",
            csv: format!(
                "label,value\n{}",
                (0..1024)
                    .map(|i| format!("row,{}\n", i + 1))
                    .collect::<String>()
            ),
            grains: vec![64, 127],
            expected: "{\"count\":1024,\"sum\":524800,\"min\":1,\"max\":1024}\n",
            error: false,
        }],
        _ => panic!("unknown partition"),
    }
}
