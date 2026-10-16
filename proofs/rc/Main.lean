import Rc.Trace

/-- `rc-trace <vector>`: evaluate a versioned trace vector and print the exact final model
state (see `Rc/Trace.lean` for the format). Exit status 0 when the vector was read, whether
or not the trace was legal; 2 on a usage or format error. -/
def main (args : List String) : IO UInt32 := do
  match args with
  | [path] =>
    let text ← IO.FS.readFile path
    match Rc.Trace.parse text with
    | .ok v =>
      IO.print (Rc.Trace.evaluate v)
      return 0
    | .error e =>
      IO.eprintln s!"rc-trace: {path}: {e}"
      return 2
  | _ =>
    IO.eprintln "usage: rc-trace <vector>"
    return 2
