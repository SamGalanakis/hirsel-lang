//! Representation adapters for pure mathematical integer conversions.
use crate::scalar::ScalarError;
use crate::value::{Integer, integer_layout};
use hirsel_kernels::integer::{self, Format, Width};
use hirsel_types::Primitive;
fn format(kind: Primitive) -> Result<Format, ScalarError> {
    let (width, signed) = integer_layout(kind).ok_or(ScalarError::InvalidOperation)?;
    Ok(Format {
        width: Width::from_bits(width).unwrap(),
        signed,
    })
}
pub fn exact(value: Integer, destination: Primitive) -> Result<Option<Integer>, ScalarError> {
    Ok(
        integer::exact(value.bits(), format(value.kind())?, format(destination)?)
            .and_then(|bits| Integer::new(destination, bits)),
    )
}
pub fn truncate(value: Integer, destination: Primitive) -> Result<Integer, ScalarError> {
    Ok(Integer::new(
        destination,
        integer::truncate(value.bits(), format(value.kind())?, format(destination)?),
    )
    .unwrap())
}
