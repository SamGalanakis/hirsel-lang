use super::*;
use crate::build::Layouts;
use crate::ledger::{Count, Owner};
use crate::{Config, NoOperations};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::Rc;

/// One row of `Number(u64) | Other(u64)` in columns of capacity 8: a u8 tag column at 16,
/// then the payload column at 16 + 8.
#[test]
fn variant_rows_read_their_payload_only_through_the_tagged_arm() {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let number = layouts.scalar(ScalarKind::U64);
    let row = layouts.variant(&[&[number], &[number]]);
    let columns = layouts.record(&[]);
    layouts.get_mut(columns).kind = LayoutKind::SoaColumns { element: row };
    let (target, layouts) = layouts.finish();
    let program = Program::<Rc> {
        target,
        layouts,
        units: vec![],
        statics: vec![],
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
    };
    let config = Config::default();
    let mut ops = NoOperations;
    let mut machine = Machine::new(&program, &config, &mut ops);
    let mut bytes = vec![0u8; 16 + 8 * 9];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    bytes[8..16].copy_from_slice(&8u64.to_le_bytes());
    bytes[16] = 1;
    bytes[24..32].copy_from_slice(&42u64.to_le_bytes());
    let ptr = machine.ledger.alloc(
        Owner::Heap,
        Some(columns),
        bytes.len() as u64,
        bytes.len() as u64,
        8,
        Some(Count::Finite(1)),
    );
    machine
        .ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(0, &bytes);
    let r = machine.column_resolved(ptr, row, 0).unwrap();
    assert_eq!(machine.column_variant(&r).unwrap(), 1);
    // The tagged arm reads the payload column's bytes.
    let Ok((at, layout)) = machine.column_variant_field(&r, 1, 0, false).unwrap() else {
        panic!("the active arm is readable")
    };
    assert_eq!(layout, number);
    assert_eq!(machine.read_scalar(at, 8).unwrap(), 42);
    // Another arm is a language failure, and a checker violation when claimed proved.
    assert!(matches!(
        machine.column_variant_field(&r, 0, 0, false),
        Ok(Err(LangFailure))
    ));
    let Err(violation) = machine.column_variant_field(&r, 0, 0, true) else {
        panic!("a proved projection of an inactive arm is a violation")
    };
    assert_eq!(violation.kind, V::TypeConfusion);
    // A gathered row widens the u8 tag into its u32 tag word.
    let image = machine.read_resolved(&r, row, false).unwrap();
    assert_eq!(image.active(0, TagForm::Ordinal, 8).unwrap(), 1);

    // A reserve's spare row has no tag or payload. Widening the stored u8
    // tag must not manufacture initialized high bytes in its u32 image.
    machine.clear_column_spare(ptr, row).unwrap();
    let spare = machine.column_resolved(ptr, row, 1).unwrap();
    let image = machine.read_resolved(&spare, row, false).unwrap();
    assert_eq!(machine.initialized(&image, 0, row).unwrap(), (false, false));
    // A real tag in a spare row is still live even without its payload:
    // SeqSetLen must continue to reject abandoning that partial write.
    machine
        .ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(17, &[1]);
    let image = machine.read_resolved(&spare, row, false).unwrap();
    assert_eq!(machine.initialized(&image, 0, row).unwrap(), (false, true));
}
