//! Canonical snapshot encodings of the contract types (hirsel-codec).
use crate::{
    CallOwnership, CleanupEffect, Disposition, EngineDispositions, M2Feature, Manifest,
    OperationId, OperationRow, ParameterOwnership, Passing, RuleCode, SemanticEffects,
    TestVectorId, Unavailable,
};
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer, snapshot_enum, snapshot_struct};

snapshot_enum!(Passing { Borrow, Take, Mut });
snapshot_struct!(OperationId {
    definition,
    declaration_digest
});
snapshot_enum!(Disposition {
    DeclarationOnly,
    Body,
    Instruction(name),
    RuntimePrimitive(symbol),
    Unavailable(reason),
});
snapshot_enum!(Unavailable {
    Unclassified,
    M2(feature),
    OpenSpec(issue),
    NotImplemented
});
snapshot_enum!(M2Feature {
    Tasks,
    Channels,
    Mutexes,
    Atomics,
    Shared,
    Arenas,
    Regions,
    Slots
});
snapshot_enum!(CleanupEffect {
    NoAlloc,
    MayAllocate,
    DependsOn(parameters)
});
snapshot_struct!(ParameterOwnership {
    ordinal,
    passing,
    cleanup
});
snapshot_struct!(CallOwnership {
    parameters,
    result_cleanup
});
snapshot_struct!(SemanticEffects {
    reads,
    writes,
    owns,
    may_fail,
    allocates,
    observable
});
impl Snapshot for TestVectorId {
    fn write(&self, w: &mut Writer) {
        w.text(&self.0);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(r.text()?))
    }
}
snapshot_struct!(EngineDispositions {
    reference,
    native,
    heap_check
});
impl Snapshot for RuleCode {
    fn write(&self, w: &mut Writer) {
        w.text(&self.0);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(r.text()?))
    }
}
snapshot_struct!(OperationRow {
    id,
    ownership,
    effects,
    dispositions,
    vectors,
    failure
});
/// Manifests encode as their rows; decoding rebuilds the fixed row set.
impl Snapshot for Manifest {
    fn write(&self, w: &mut Writer) {
        w.open("Manifest");
        for row in self.rows() {
            row.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.open("Manifest")?;
        let mut rows = Vec::new();
        while !r.at_close() {
            rows.push(OperationRow::read(r)?);
        }
        r.close()?;
        let mut manifest = Manifest::skeleton(rows.iter().map(|row| row.id.clone()));
        if manifest.len() != rows.len() {
            return Err(r.error("duplicate manifest row"));
        }
        for row in rows {
            manifest
                .replace(row)
                .map_err(|_| r.error("manifest row outside its identity set"))?;
        }
        Ok(manifest)
    }
}
