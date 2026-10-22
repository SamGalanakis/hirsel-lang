//! LangArena brainfuck_recursion.rs:4-104, 3c9f84ddaeec834c0631747cb789e8cfd602f993.
//! Copyright (c) 2026 Konstantin Makarchev; MIT, see LICENSE in this directory.
//! Diagnostic adapter: retain parser/Tape/execution order, expose final tape,
//! emitted bytes, checksum, actual recursive calls and visited Op counts.
//! One bounded program per stdin line; full run.js also works natively.
enum Op {
    Dec,
    Inc,
    Prev,
    Next,
    Print,
    Loop(Vec<Op>),
}

struct Tape {
    pos: usize,
    tape: Vec<u8>,
    output: Vec<u8>,
    calls: u64,
    visits: u64,
}

impl Tape {
    fn new() -> Self {
        Self {
            pos: 0,
            tape: vec![0],
            output: Vec::new(),
            calls: 0,
            visits: 0,
        }
    }

    fn current_cell(&self) -> u8 {
        self.tape[self.pos]
    }

    fn inc(&mut self) {
        self.tape[self.pos] = self.tape[self.pos].wrapping_add(1);
    }

    fn dec(&mut self) {
        self.tape[self.pos] = self.tape[self.pos].wrapping_sub(1);
    }

    fn prev(&mut self) {
        if self.pos > 0 {
            self.pos -= 1;
        }
    }

    fn next(&mut self) {
        self.pos += 1;
        if self.pos >= self.tape.len() {
            self.tape.resize(self.pos + 1, 0);
        }
    }
}

struct Program {
    ops: Vec<Op>,
}

impl Program {
    fn new(code: &str) -> Self {
        Self {
            ops: Self::parse(&mut code.bytes()),
        }
    }

    fn parse(iter: &mut impl Iterator<Item = u8>) -> Vec<Op> {
        let mut buf = Vec::new();
        while let Some(byte) = iter.next() {
            match byte {
                b'-' => buf.push(Op::Dec),
                b'+' => buf.push(Op::Inc),
                b'<' => buf.push(Op::Prev),
                b'>' => buf.push(Op::Next),
                b'.' => buf.push(Op::Print),
                b'[' => buf.push(Op::Loop(Self::parse(iter))),
                b']' => break,
                _ => continue,
            }
        }
        buf
    }

    fn run(&self) -> i64 {
        let mut tape = Tape::new();
        let mut result = 0i64;
        self.execute(&self.ops, &mut tape, &mut result);
        result
    }

    fn execute(&self, program: &[Op], tape: &mut Tape, result: &mut i64) {
        tape.calls += 1;
        for op in program {
            tape.visits += 1;
            match op {
                Op::Dec => tape.dec(),
                Op::Inc => tape.inc(),
                Op::Prev => tape.prev(),
                Op::Next => tape.next(),
                Op::Print => {
                    tape.output.push(tape.current_cell());
                    *result = ((*result << 2) as i64).wrapping_add(tape.current_cell() as i64);
                }
                Op::Loop(inner) => {
                    while tape.current_cell() != 0 {
                        self.execute(inner, tape, result);
                    }
                }
            }
        }
    }
}

fn main() {
    use std::io::BufRead;
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let program = Program::new(&line);
        let mut tape = Tape::new();
        let mut result = 0i64;
        program.execute(&program.ops, &mut tape, &mut result);
        println!(
            "{{\"cells\":{:?},\"pos\":{},\"output\":{:?},\"checksum\":{},\"calls\":{},\"visits\":{}}}",
            tape.tape, tape.pos, tape.output, result as u32, tape.calls, tape.visits
        );
    }
}
