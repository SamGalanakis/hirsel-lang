//! Wycheproof's valid/invalid AEAD contract, the GCM specification and RFC 8439.
use super::reference_vectors::{self as vectors, Case, bytes};
use serde_json::Value;

const AES: &str = include_str!("../../../../native-tests/library/vectors/aes_gcm_test.json");
const CHACHA: &str =
    include_str!("../../../../native-tests/library/vectors/chacha20_poly1305_test.json");
const XCHACHA: &str =
    include_str!("../../../../native-tests/library/vectors/xchacha20_poly1305_test.json");

fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    row[name].as_str().unwrap()
}

fn aead_case(module: &str, row: &Value) -> Case {
    let id = row["tcId"].as_u64().unwrap();
    let key = bytes(field(row, "key"));
    let iv = bytes(field(row, "iv"));
    let aad = bytes(field(row, "aad"));
    let msg = bytes(field(row, "msg"));
    let sealed = bytes(&format!("{}{}", field(row, "ct"), field(row, "tag")));
    let open = format!("std@1/{module}.open({key}, {iv}, {sealed}, {aad})");
    let body = match field(row, "result") {
        "valid" => format!(
            "core@1/test.check(t, std@1/{module}.seal({key}, {iv}, {msg}, {aad}) == {sealed})\n\
             match {open} {{ Ok(plaintext) => core@1/test.check(t, plaintext == {msg}), Error(_) => core@1/test.check(t, false) }}"
        ),
        "invalid" => format!(
            "match {open} {{ Error(AuthenticationFailed()) => (), Ok(_) => core@1/test.check(t, false) }}"
        ),
        result => panic!("{module} tcId={id}: unhandled result {result}"),
    };
    Case {
        id: format!(
            "{module} tcId={id} {}",
            row["comment"].as_str().unwrap_or("")
        ),
        body,
    }
}

fn wycheproof(module: &str, json: &str, nonce_bits: u64, count: usize, shard: usize) {
    let file: Value = serde_json::from_str(json).unwrap();
    let groups = file["testGroups"].as_array().unwrap();
    assert_eq!(
        groups
            .iter()
            .map(|g| g["tests"].as_array().unwrap().len())
            .sum::<usize>(),
        file["numberOfTests"].as_u64().unwrap() as usize
    );
    let cases: Vec<_> = groups
        .iter()
        .filter(|g| g["ivSize"] == nonce_bits && g["tagSize"] == 128)
        .flat_map(|g| g["tests"].as_array().unwrap())
        .map(|row| aead_case(module, row))
        .collect();
    assert_eq!(
        cases.len(),
        count,
        "{module}: applicable vector count changed"
    );
    vectors::run(module, &cases, shard, 8, 8, "");
}

// Separate Rust tests let the normal it-stdlib cost-balanced shards distribute the work.
macro_rules! shards {
    ($module:literal, $file:ident, $bits:literal, $count:literal; $($name:ident: $shard:literal),+ $(,)?) => {
        $(#[test] fn $name() { wycheproof($module, $file, $bits, $count, $shard); })+
    };
}
shards!("aes_gcm", AES, 96, 197;
    aes_gcm_0: 0, aes_gcm_1: 1, aes_gcm_2: 2, aes_gcm_3: 3,
    aes_gcm_4: 4, aes_gcm_5: 5, aes_gcm_6: 6, aes_gcm_7: 7);
shards!("chacha20poly1305", CHACHA, 96, 316;
    chacha20poly1305_0: 0, chacha20poly1305_1: 1, chacha20poly1305_2: 2, chacha20poly1305_3: 3,
    chacha20poly1305_4: 4, chacha20poly1305_5: 5, chacha20poly1305_6: 6, chacha20poly1305_7: 7);
shards!("xchacha20poly1305", XCHACHA, 192, 306;
    xchacha20poly1305_0: 0, xchacha20poly1305_1: 1, xchacha20poly1305_2: 2, xchacha20poly1305_3: 3,
    xchacha20poly1305_4: 4, xchacha20poly1305_5: 5, xchacha20poly1305_6: 6, xchacha20poly1305_7: 7);

#[test]
fn aes_gcm_spec_twelve_96_bit_iv_cases() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../native-tests/library/vectors/gcm-spec.json"
    ))
    .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["tcId"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 7, 8, 9, 10, 13, 14, 15, 16]
    );
    let cases: Vec<_> = rows.iter().map(|row| aead_case("aes_gcm", row)).collect();
    vectors::run("GCM specification appendix B", &cases, 0, 1, 4, "");
}

#[test]
fn internal_chacha_poly_rfc8439_a1_a2_a3() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../native-tests/library/vectors/rfc8439.json"
    ))
    .unwrap();
    assert_eq!(rows.len(), 19); // A.1: five; A.2: three; A.3: eleven.
    let cases: Vec<_> = rows
        .iter()
        .map(|row| {
            let id = field(row, "id");
            let key = bytes(field(row, "key"));
            let msg = bytes(field(row, "msg"));
            let expected = bytes(field(row, "expected"));
            let call = if id.starts_with("A.3.") {
                format!("primitive_mac({key}, {msg})")
            } else {
                let iv = bytes(field(row, "iv"));
                let counter = row["counter"].as_u64().unwrap();
                format!("xor({key}, {iv}, {counter}u32, {msg})")
            };
            Case {
                id: id.into(),
                body: format!("core@1/test.check(t, {call} == {expected})"),
            }
        })
        .collect();
    // Compile the actual internal source verbatim in the fixture's module: private
    // primitives remain private. This adapter supplies RFC 8439 section 2.5's
    // final partial block, which AEAD deliberately never uses (it zero-pads).
    let source = format!(
        "{}\n{}",
        include_str!("../../../../stdlib/std@1/chacha_poly.hir"),
        include_str!("primitive-poly1305.hir")
    );
    vectors::run("RFC 8439 A.1-A.3", &cases, 0, 1, 4, &source);
}
