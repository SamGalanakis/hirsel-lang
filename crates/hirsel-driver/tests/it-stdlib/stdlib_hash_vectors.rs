//! Go golden/KAT loops and published RFC/Wycheproof MAC and KDF rows.
use super::reference_vectors::{self as vectors, Case, bytes};
use serde_json::Value;

const GO_BLAKE2B_KEYED: &str =
    include_str!("../../../../native-tests/library/vectors/go-blake2b-keyed.json");
const GO_BLAKE2S_KEYED: &str =
    include_str!("../../../../native-tests/library/vectors/go-blake2s-keyed.json");
const HMAC_SHA1_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha1_test.json");
const HMAC_SHA224_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha224_test.json");
const HMAC_SHA256_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha256_test.json");
const HMAC_SHA384_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha384_test.json");
const HMAC_SHA512_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha512_test.json");
const HMAC_SHA512_256_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha512_256_test.json");
const HMAC_SHA3_224_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha3_224_test.json");
const HMAC_SHA3_256_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha3_256_test.json");
const HMAC_SHA3_384_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha3_384_test.json");
const HMAC_SHA3_512_TEST: &str =
    include_str!("../../../../native-tests/library/vectors/hmac_sha3_512_test.json");

fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    row[name].as_str().unwrap()
}

fn rows(json: &str) -> Vec<Value> {
    serde_json::from_str(json).unwrap()
}

fn golden(module: &str, count: usize) {
    let cases: Vec<_> = rows(include_str!("../../../../native-tests/library/vectors/go-hash-golden.json"))
        .iter()
        .filter(|row| row["hash"] == module)
        .map(|row| {
            let msg = field(row, "msg");
            let mid = (msg.len() / 4) * 2;
            let expected = bytes(field(row, "expected"));
            Case {
                id: field(row, "id").into(),
                body: format!(
                    "core@1/test.check(t, std@1/bytes.from_inline(std@1/{module}.digest({})) == {expected})\n\
                     let mut state = std@1/{module}.new()\n\
                     std@1/{module}.update(mut state, {})\n\
                     std@1/{module}.update(mut state, b\"\")\n\
                     std@1/{module}.update(mut state, {})\n\
                     core@1/test.check(t, std@1/bytes.from_inline(std@1/{module}.finish(take state)) == {expected})",
                    bytes(msg), bytes(&msg[..mid]), bytes(&msg[mid..])
                ),
            }
        })
        .collect();
    assert_eq!(cases.len(), count);
    vectors::run(module, &cases, 0, 1, 8, "");
}

#[test]
fn sha1_go_golden() {
    golden("sha1", 32);
}
#[test]
fn sha224_go_golden() {
    golden("sha224", 31);
}
#[test]
fn sha256_go_golden() {
    golden("sha256", 31);
}
#[test]
fn sha384_go_golden() {
    golden("sha384", 31);
}
#[test]
fn sha512_go_golden() {
    golden("sha512", 31);
}
#[test]
fn sha512_256_go_golden() {
    golden("sha512_256", 31);
}
#[test]
fn md5_go_golden() {
    golden("md5", 31);
}

fn blake(module: &str, json: &str, key_len: u8, shard: usize) {
    let expected: Vec<String> = serde_json::from_str(json).unwrap();
    assert_eq!(expected.len(), 256);
    let key = bytes(&(0..key_len).map(|b| format!("{b:02x}")).collect::<String>());
    let cases: Vec<_> = expected.iter().enumerate().map(|(i, sum)| {
        let msg = bytes(&(0..i).map(|b| format!("{b:02x}")).collect::<String>());
        let expected = bytes(sum);
        Case {
            id: format!("{module}.hashes[{i}]"),
            body: format!(
                "let mut state = std@1/{module}.new_keyed({key})\n\
                 std@1/{module}.update(mut state, {msg})\n\
                 core@1/test.check(t, std@1/bytes.from_inline(std@1/{module}.finish(take state)) == {expected})\n\
                 let mut stream = std@1/{module}.new_keyed({key})\n\
                 let msg = {msg}\n\
                 for j in 0u64..{i}u64 {{ std@1/{module}.update(mut stream, core@1/bytes.slice(msg, j, j + 1)) }}\n\
                 core@1/test.check(t, std@1/bytes.from_inline(std@1/{module}.finish(take stream)) == {expected})"
            ),
        }
    }).collect();
    vectors::run(module, &cases, shard, 4, 8, "");
}
macro_rules! blake_shards {
    ($module:literal, $file:ident, $len:literal; $($name:ident: $shard:literal),+) => {
        $(#[test] fn $name() { blake($module, $file, $len, $shard); })+
    };
}
blake_shards!("blake2b", GO_BLAKE2B_KEYED, 64;
    blake2b_0: 0, blake2b_1: 1, blake2b_2: 2, blake2b_3: 3);
blake_shards!("blake2s", GO_BLAKE2S_KEYED, 32;
    blake2s_0: 0, blake2s_1: 1, blake2s_2: 2, blake2s_3: 3);

fn hmac_case(row: &Value, module: &str, id: String) -> Case {
    let key = bytes(field(row, "key"));
    let msg = bytes(field(row, "msg"));
    let tag = field(row, "tag");
    let len = tag.len() / 2;
    // MD5 exposes the digest operations but no convenience dictionary constructor.
    let hash = if module == "md5" {
        "std@1/digest.Hash(new: std@1/md5.new, update: std@1/md5.update, finish: std@1/md5.finish, block_size: 64)".into()
    } else {
        format!("std@1/{module}.hash")
    };
    let mut body = format!(
        "let actual = std@1/bytes.from_inline(std@1/hmac.mac({hash}, {key}, {msg}))\n\
         core@1/test.check(t, core@1/bytes.slice(actual, 0, {len}u64) == {})",
        bytes(tag)
    );
    if !id.starts_with("RFC4231.5.") {
        body.push_str(&format!(
            "\ncore@1/test.check(t, std@1/hmac.verify({hash}, {key}, {msg}, {}))",
            bytes(tag)
        ));
    }
    if let Some(tag96) = row["tag96"].as_str() {
        body.push_str(&format!(
            "\ncore@1/test.check(t, core@1/bytes.slice(actual, 0, 12) == {})",
            bytes(tag96)
        ));
    }
    Case { id, body }
}

#[test]
fn hmac_rfc4231_and_rfc2202() {
    let data = rows(include_str!(
        "../../../../native-tests/library/vectors/rfc-hmac.json"
    ));
    assert_eq!(data.len(), 42); // 7 x 4 SHA-2 + 7 SHA-1 + 7 MD5.
    let cases: Vec<_> = data
        .iter()
        .map(|row| hmac_case(row, field(row, "hash"), field(row, "id").into()))
        .collect();
    vectors::run("RFC-HMAC", &cases, 0, 1, 8, "");
}

fn wycheproof_hmac(module: &str, json: &str, bits: u64) {
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
        .filter(|g| g["tagSize"] == bits)
        .flat_map(|g| g["tests"].as_array().unwrap())
        .filter(|row| row["result"] == "valid")
        .map(|row| {
            assert_eq!(field(row, "tag").len() as u64 * 4, bits);
            hmac_case(row, module, format!("{module}.tcId={}", row["tcId"]))
        })
        .collect();
    assert_eq!(cases.len(), 33);
    vectors::run(module, &cases, 0, 1, 8, "");
}
macro_rules! hmac_suite {
    ($($name:ident: $module:literal, $file:ident, $bits:literal);+ $(;)?) => {
        $(#[test] fn $name() { wycheproof_hmac($module, $file, $bits); })+
    };
}
hmac_suite!(
    hmac_sha1: "sha1", HMAC_SHA1_TEST, 160;
    hmac_sha224: "sha224", HMAC_SHA224_TEST, 224;
    hmac_sha256: "sha256", HMAC_SHA256_TEST, 256;
    hmac_sha384: "sha384", HMAC_SHA384_TEST, 384;
    hmac_sha512: "sha512", HMAC_SHA512_TEST, 512;
    hmac_sha512_256: "sha512_256", HMAC_SHA512_256_TEST, 256;
    hmac_sha3_224: "sha3_224", HMAC_SHA3_224_TEST, 224;
    hmac_sha3_256: "sha3_256", HMAC_SHA3_256_TEST, 256;
    hmac_sha3_384: "sha3_384", HMAC_SHA3_384_TEST, 384;
    hmac_sha3_512: "sha3_512", HMAC_SHA3_512_TEST, 512;
);

fn pbkdf2_case(module: &str, row: &Value, id: String) -> Case {
    let dk = field(row, "dk");
    Case {
        id,
        body: format!(
            "core@1/test.check(t, std@1/pbkdf2.derive(std@1/{module}.hash, {}, {}, {}u64, {}u64) == {})",
            bytes(field(row, "password")),
            bytes(field(row, "salt")),
            row["iterationCount"].as_u64().unwrap(),
            dk.len() / 2,
            bytes(dk)
        ),
    }
}

fn wycheproof_pbkdf2(module: &str, json: &str, count: usize) {
    let file: Value = serde_json::from_str(json).unwrap();
    let data: Vec<_> = file["testGroups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["tests"].as_array().unwrap())
        .collect();
    assert_eq!(data.len(), count);
    assert_eq!(data.len(), file["numberOfTests"].as_u64().unwrap() as usize);
    let mut skipped = vec![];
    let cases: Vec<_> = data
        .iter()
        .filter_map(|row| {
            assert_eq!(row["result"], "valid");
            assert_eq!(
                row["dkLen"].as_u64().unwrap() as usize * 2,
                field(row, "dk").len()
            );
            if module == "sha1" && row["tcId"] == 4 {
                // RFC 6070's sole excluded row, also excluded by Go: 2^24 rounds.
                assert_eq!(row["iterationCount"], 16_777_216);
                skipped.push(4);
                None
            } else {
                Some(pbkdf2_case(
                    module,
                    row,
                    format!("{module}.tcId={}", row["tcId"]),
                ))
            }
        })
        .collect();
    assert_eq!(skipped, if module == "sha1" { vec![4] } else { vec![] });
    vectors::run_native(&format!("PBKDF2-{module}"), &cases, 16);
}

#[test]
fn pbkdf2_sha1() {
    wycheproof_pbkdf2(
        "sha1",
        include_str!("../../../../native-tests/library/vectors/pbkdf2_hmacsha1_test.json"),
        64,
    );
}
#[test]
fn pbkdf2_sha256() {
    wycheproof_pbkdf2(
        "sha256",
        include_str!("../../../../native-tests/library/vectors/pbkdf2_hmacsha256_test.json"),
        60,
    );
}
#[test]
fn pbkdf2_sha512() {
    wycheproof_pbkdf2(
        "sha512",
        include_str!("../../../../native-tests/library/vectors/pbkdf2_hmacsha512_test.json"),
        58,
    );
}

#[test]
fn pbkdf2_go_and_rfc6070_real_work_factors() {
    let data = rows(include_str!(
        "../../../../native-tests/library/vectors/go-pbkdf2.json"
    ));
    assert_eq!(data.len(), 10);
    assert_eq!(
        data.iter()
            .filter(|row| row["hash"] == "sha1")
            .map(|row| row["iterationCount"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 4096, 4096, 4096]
    );
    let cases: Vec<_> = data
        .iter()
        .map(|row| pbkdf2_case(field(row, "hash"), row, field(row, "id").into()))
        .collect();
    vectors::run_native("PBKDF2-Go-RFC6070", &cases, 10);
}

#[test]
fn adler32_go_golden_window_and_long_rows() {
    let data = rows(include_str!(
        "../../../../native-tests/library/vectors/go-adler32-golden.json"
    ));
    assert_eq!(data.len(), 43);
    let cases: Vec<_> = data.iter().map(|row| {
        let pattern = bytes(field(row, "repeat"));
        let suffix = bytes(field(row, "suffix"));
        let count = row["count"].as_u64().unwrap();
        let expected = field(row, "expected");
        Case { id: field(row, "id").into(), body: format!(
            "let mut data: List[u8] = []\n\
             for i in 0u64..{count}u64 {{ for byte in {pattern} {{ core@1/list.push(mut data, byte) }} }}\n\
             for byte in {suffix} {{ core@1/list.push(mut data, byte) }}\n\
             let msg = core@1/bytes.from_list(take data)\n\
             core@1/test.check(t, std@1/adler32.digest(msg) == 0x{expected}u32)\n\
             let mut state = std@1/adler32.new()\n\
             let mid = core@1/bytes.len(msg) / 2\n\
             std@1/adler32.update(mut state, core@1/bytes.slice(msg, 0, mid))\n\
             std@1/adler32.update(mut state, core@1/bytes.slice(msg, mid, core@1/bytes.len(msg)))\n\
             core@1/test.check(t, std@1/adler32.finish(take state) == 0x{expected}u32)"
        ) }
    }).collect();
    vectors::run_native("Adler32-Go", &cases, 16);
}
