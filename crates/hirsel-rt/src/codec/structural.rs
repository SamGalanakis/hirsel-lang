//! MEM-3 preflight over emitted words and a bounded sibling cursor stack.
use super::borrowed::{Program, Projection, children};
use crate::fault;
use std::mem::MaybeUninit;

const LIMIT: u64 = hirsel_core_contract::depth::STRUCTURAL_LIMIT;

/// The program is independently validated and its operand stays borrowed.
/// Only branching logical nodes retain a sibling frame. Unary representation
/// wrappers use tail steps, so neither width nor transparent type-graph depth
/// imposes a new limit. The over-limit child is rejected before it is read.
pub(crate) unsafe fn preflight(words: &[u64], edges: &[u8], value: *const u8) -> bool {
    let program = Program::new(words);
    if edges.len() != program.count() {
        fault("malformed structural edge table");
    }
    let mut stack = [const { MaybeUninit::uninit() }; LIMIT as usize + 1];
    let mut used = 0;
    let mut current = (Projection::at(program.root(), value), 0u64);
    loop {
        let (projection, depth) = current;
        if depth > LIMIT {
            return false;
        }
        let edge = edges[projection.node];
        if edge > 1 {
            fault("malformed structural edge");
        }
        let next = depth + u64::from(edge);
        // SAFETY: every child is a borrowed projection by the validated program.
        unsafe {
            let mut cursor = children(program, projection);
            if let Some(child) = cursor.next(program) {
                if cursor.has_more() {
                    if used == stack.len() {
                        fault("structural sibling stack invariant");
                    }
                    stack[used].write((cursor, next));
                    used += 1;
                }
                current = (child, next);
                continue;
            }
            loop {
                if used == 0 {
                    return true;
                }
                let (mut cursor, depth) = stack[used - 1].assume_init_read();
                if let Some(child) = cursor.next(program) {
                    if cursor.has_more() {
                        stack[used - 1].write((cursor, depth));
                    } else {
                        used -= 1;
                    }
                    current = (child, depth);
                    break;
                }
                used -= 1;
            }
        }
    }
}
