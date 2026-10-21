#!/usr/bin/env python3
"""Compare checked-in spans using pinned upstream Go regexp, not a host regex.

Usage: python3 regex-all-5-oracle.py /tmp/ref-go /path/to/go
The Go executable only builds; regexp and regexp/syntax come from the pin.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

PIN = "ebe2feefc400f813aefdc8a036e66d2619da6649"
reference = Path(sys.argv[1]).resolve()
go = str(Path(sys.argv[2]).resolve())
actual = subprocess.check_output(
    ["git", "-C", str(reference), "rev-parse", "HEAD"], text=True
).strip()
assert actual == PIN, (actual, PIN)
vectors = Path(__file__).with_name("regex-all-5-vectors.json").resolve()
with tempfile.TemporaryDirectory(prefix="regex-all-5-oracle-") as temporary:
    root = Path(temporary)
    (root / "go.mod").write_text("module regexoracle\n\ngo 1.25.1\n")
    source = reference / "src/regexp"
    for directory in [source, source / "syntax"]:
        target = root / directory.relative_to(source)
        target.mkdir(exist_ok=True)
        for path in directory.glob("*.go"):
            if not path.name.endswith("_test.go"):
                (target / path.name).write_text(
                    path.read_text().replace('"regexp/syntax"', '"regexoracle/syntax"')
                )
    (root / "cmd").mkdir()
    (root / "cmd/main.go").write_text(r'''
package main
import (
 "encoding/json"
 "fmt"
 "os"
 "reflect"
 regexp "regexoracle"
)
func main() {
 var rows []struct { OracleIndex int `json:"oracle_index"`; Pattern string; Text string; Max int; Spans [][]int }
 data,err:=os.ReadFile(os.Args[1]); if err!=nil {panic(err)}
 if err=json.Unmarshal(data,&rows); err!=nil {panic(err)}
 for _,row:=range rows {
  got:=regexp.MustCompile(row.Pattern).FindAllStringIndex(row.Text,row.Max)
  if !reflect.DeepEqual(got,row.Spans) {panic(fmt.Sprintf("vector %d: got %v, want %v",row.OracleIndex,got,row.Spans))}
 }
 fmt.Printf("%d pinned Go span vectors agree\n",len(rows))
}
''')
    subprocess.run([go, "run", "./cmd", str(vectors)], cwd=root, check=True)
print(f"Go source {PIN}; {subprocess.check_output([go, 'version'], text=True).strip()}")
