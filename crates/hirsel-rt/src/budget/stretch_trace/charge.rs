#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Charge {
    Fuel(u64),
    Alloc(u64),
    Free(u64),
    Credit(u64),
}
