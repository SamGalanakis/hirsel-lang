//! What lowering one unit does to the program besides producing its LIR (design §1e): the
//! layouts, `FnAbi`s and environments it derives and the library instances it requires, in
//! the order it asks for them. A unit whose object the object store holds is not imported:
//! its recorded effects are replayed instead, so the program's layout table and instance
//! list are the ones importing it would have built, in the same order, and every layout
//! and callee its object names exists.
//!
//! Effects name types by session `TypeId`, definitions by `DefId` and layouts the unit
//! derived by the entry that derived them, so they serialize inside
//! `hirsel_types::with_table_encoding` over [`Effects::types`]. A call the recorder cannot
//! name that way makes the effects unreplayable: such a unit is always imported.

use crate::instance::{Actuals, InstanceError, Instances};
use crate::library::Request;
use crate::library::builder::Layouts;
use hirsel_core_contract::Passing;
use hirsel_layout::layout::{EnvironmentShape, Owners};
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::layout::LayoutError;
use hirsel_lir::layout_types::{LayoutId, LayoutRecord, LayoutTable};
use hirsel_syntax::{DefId, DefPath, SiteRef};
use hirsel_types::{GenericArgument, TypeId, TypeInterner};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One builder call or instance request, in the order the unit made it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    Layout(TypeId),
    FnAbi {
        callable: TypeId,
        shape: Shape,
    },
    /// `with_caller_site` of the `FnAbi` entry at this index.
    CallerSite(u32),
    CursorSource,
    Environment {
        definition: DefId,
        body: u32,
        captures: Vec<(TypeId, Convention)>,
        sizes: Vec<u64>,
        movable: Vec<u32>,
    },
    FlatBuilder(LayoutRef),
    Request(RequestRecord),
}

/// A layout an effect reads: one an earlier entry derived, or the environment of the
/// unit's own instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LayoutRef {
    Entry(u32),
    OwnEnvironment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Definition,
    Closure,
    IterNext,
    IterDrop,
    Foreign,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Convention {
    Borrow,
    Take,
    Mut,
}

/// A library instance request (`crate::library::Request`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestRecord {
    pub body: u32,
    pub path: DefPath,
    pub binder: Option<SiteRef>,
    pub arguments: Vec<GenericArgument>,
    pub outer: Vec<(SiteRef, Vec<GenericArgument>)>,
    pub callable: TypeId,
    pub shape: Shape,
    pub environment: Option<LayoutRef>,
    pub finalizer: bool,
}

/// The effects of lowering one unit.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    pub entries: Vec<Effect>,
}

impl Effects {
    /// Every type the effects name: the roots their table encoding closes over.
    pub fn types(&self) -> Vec<TypeId> {
        let mut out = Vec::new();
        let arguments = |out: &mut Vec<TypeId>, arguments: &[GenericArgument]| {
            out.extend(arguments.iter().filter_map(|a| match a {
                GenericArgument::Type(t) => Some(*t),
                GenericArgument::Size(_) => None,
            }))
        };
        for entry in &self.entries {
            match entry {
                Effect::Layout(t) | Effect::FnAbi { callable: t, .. } => out.push(*t),
                Effect::Environment { captures, .. } => {
                    out.extend(captures.iter().map(|(t, _)| *t))
                }
                Effect::Request(r) => {
                    out.push(r.callable);
                    arguments(&mut out, &r.arguments);
                    for (_, outer) in &r.outer {
                        arguments(&mut out, outer);
                    }
                }
                Effect::CallerSite(_) | Effect::CursorSource | Effect::FlatBuilder(_) => {}
            }
        }
        out
    }
}

impl From<CallableShape> for Shape {
    fn from(shape: CallableShape) -> Self {
        match shape {
            CallableShape::Definition => Self::Definition,
            CallableShape::Closure => Self::Closure,
            CallableShape::IterNext => Self::IterNext,
            CallableShape::IterDrop => Self::IterDrop,
            CallableShape::Foreign => Self::Foreign,
        }
    }
}

impl From<Shape> for CallableShape {
    fn from(shape: Shape) -> Self {
        match shape {
            Shape::Definition => Self::Definition,
            Shape::Closure => Self::Closure,
            Shape::IterNext => Self::IterNext,
            Shape::IterDrop => Self::IterDrop,
            Shape::Foreign => Self::Foreign,
        }
    }
}

impl From<Passing> for Convention {
    fn from(passing: Passing) -> Self {
        match passing {
            Passing::Borrow => Self::Borrow,
            Passing::Take => Self::Take,
            Passing::Mut => Self::Mut,
        }
    }
}

impl From<Convention> for Passing {
    fn from(convention: Convention) -> Self {
        match convention {
            Convention::Borrow => Self::Borrow,
            Convention::Take => Self::Take,
            Convention::Mut => Self::Mut,
        }
    }
}

/// The program's layouts behind a journal of the calls one unit makes.
pub struct Recorder<'a> {
    inner: &'a mut dyn Layouts,
    /// The unit's own instance's environment layout.
    own_environment: Option<LayoutId>,
    effects: Effects,
    layouts: HashMap<LayoutId, u32>,
    abis: HashMap<FnAbiId, u32>,
    /// Why the effects cannot be replayed, when they cannot.
    unreplayable: Option<String>,
}

impl<'a> Recorder<'a> {
    pub fn new(inner: &'a mut dyn Layouts, own_environment: Option<LayoutId>) -> Self {
        Self {
            inner,
            own_environment,
            effects: Effects::default(),
            layouts: HashMap::new(),
            abis: HashMap::new(),
            unreplayable: None,
        }
    }

    fn push(&mut self, effect: Effect) -> u32 {
        let index = self.effects.entries.len() as u32;
        self.effects.entries.push(effect);
        index
    }

    fn derived(
        &mut self,
        effect: Effect,
        layout: Result<LayoutId, LayoutError>,
    ) -> Result<LayoutId, LayoutError> {
        let index = self.push(effect);
        let layout = layout?;
        self.layouts.entry(layout).or_insert(index);
        Ok(layout)
    }

    fn reference(&mut self, layout: LayoutId, what: &str) -> LayoutRef {
        if let Some(&index) = self.layouts.get(&layout) {
            return LayoutRef::Entry(index);
        }
        if self.own_environment == Some(layout) {
            return LayoutRef::OwnEnvironment;
        }
        self.unreplayable.get_or_insert_with(|| {
            format!("{what} reads layout {layout:?}, which no entry derived")
        });
        LayoutRef::Entry(u32::MAX)
    }

    fn record_request(&mut self, request: &Request) {
        let environment = request
            .environment
            .map(|layout| self.reference(layout, "a request"));
        let record = RequestRecord {
            body: request.body as u32,
            path: request.path.clone(),
            binder: request.actuals.binder,
            arguments: request.actuals.arguments.clone(),
            outer: request.actuals.outer.clone(),
            callable: request.callable,
            shape: request.shape.into(),
            environment,
            finalizer: request.finalizer,
        };
        self.push(Effect::Request(record));
    }

    /// The effects, or why they cannot be replayed.
    pub fn finish(self) -> Result<Effects, String> {
        match self.unreplayable {
            None => Ok(self.effects),
            Some(reason) => Err(reason),
        }
    }
}

impl Layouts for Recorder<'_> {
    fn layout(&mut self, types: &TypeInterner, ty: TypeId) -> Result<LayoutId, LayoutError> {
        let layout = self.inner.layout(types, ty);
        self.derived(Effect::Layout(ty), layout)
    }
    fn record(&self, id: LayoutId) -> &LayoutRecord {
        self.inner.record(id)
    }
    fn fn_abi(
        &mut self,
        types: &TypeInterner,
        callable: TypeId,
        shape: CallableShape,
    ) -> Result<FnAbiId, LayoutError> {
        let index = self.push(Effect::FnAbi {
            callable,
            shape: shape.into(),
        });
        let abi = self.inner.fn_abi(types, callable, shape)?;
        self.abis.entry(abi).or_insert(index);
        Ok(abi)
    }
    fn with_caller_site(&mut self, base: FnAbiId) -> Result<FnAbiId, LayoutError> {
        match self.abis.get(&base) {
            Some(&index) => {
                let at = self.push(Effect::CallerSite(index));
                let abi = self.inner.with_caller_site(base)?;
                self.abis.entry(abi).or_insert(at);
                Ok(abi)
            }
            None => {
                self.unreplayable.get_or_insert_with(|| {
                    format!("with_caller_site of {base:?}, which no entry derived")
                });
                self.inner.with_caller_site(base)
            }
        }
    }
    fn table(&self) -> &LayoutTable {
        self.inner.table()
    }
    fn owners(&self) -> &Owners {
        self.inner.owners()
    }
    fn cursor_source(&mut self) -> LayoutId {
        let layout = self.inner.cursor_source();
        self.derived(Effect::CursorSource, Ok(layout))
            .expect("a cursor source")
    }
    fn environment(
        &mut self,
        types: &TypeInterner,
        shape: &EnvironmentShape<'_>,
    ) -> Result<LayoutId, LayoutError> {
        let effect = Effect::Environment {
            definition: shape.definition,
            body: shape.body,
            captures: shape
                .captures
                .iter()
                .map(|&(t, passing)| (t, passing.into()))
                .collect(),
            sizes: shape.sizes.to_vec(),
            movable: shape.movable.to_vec(),
        };
        let layout = self.inner.environment(types, shape);
        self.derived(effect, layout)
    }
    fn request(&mut self, request: &Request) {
        self.record_request(request);
    }
    fn flat_builder(&mut self, element: LayoutId) -> Result<LayoutId, LayoutError> {
        let element_ref = self.reference(element, "a flat builder");
        let layout = self.inner.flat_builder(element);
        self.derived(Effect::FlatBuilder(element_ref), layout)
    }
}

/// Why replaying effects failed: a tool error, since they were recorded from this
/// compiler under the same `UnitKey`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayError {
    Layout(LayoutError),
    Instance(InstanceError),
    Malformed(String),
}

/// Make the builder calls and instance requests `effects` recorded, in order. `own` is
/// the replayed unit's own instance.
pub fn replay(
    effects: &Effects,
    layouts: &mut dyn Layouts,
    instances: &mut Instances,
    own_environment: Option<LayoutId>,
) -> Result<(), ReplayError> {
    let types = instances.types.clone();
    let mut derived: Vec<Option<LayoutId>> = Vec::with_capacity(effects.entries.len());
    let mut abis: Vec<Option<FnAbiId>> = Vec::with_capacity(effects.entries.len());
    let resolve = |derived: &[Option<LayoutId>], r: LayoutRef| -> Result<LayoutId, ReplayError> {
        match r {
            LayoutRef::Entry(i) => derived
                .get(i as usize)
                .copied()
                .flatten()
                .ok_or_else(|| ReplayError::Malformed(format!("entry {i} derived no layout"))),
            LayoutRef::OwnEnvironment => own_environment
                .ok_or_else(|| ReplayError::Malformed("the instance has no environment".into())),
        }
    };
    for entry in &effects.entries {
        let (layout, abi) = match entry {
            Effect::Layout(t) => (
                Some(layouts.layout(&types, *t).map_err(ReplayError::Layout)?),
                None,
            ),
            Effect::FnAbi { callable, shape } => (
                None,
                Some(
                    layouts
                        .fn_abi(&types, *callable, (*shape).into())
                        .map_err(ReplayError::Layout)?,
                ),
            ),
            Effect::CallerSite(i) => {
                let base =
                    abis.get(*i as usize).copied().flatten().ok_or_else(|| {
                        ReplayError::Malformed(format!("entry {i} derived no FnAbi"))
                    })?;
                (
                    None,
                    Some(
                        layouts
                            .with_caller_site(base)
                            .map_err(ReplayError::Layout)?,
                    ),
                )
            }
            Effect::CursorSource => (Some(layouts.cursor_source()), None),
            Effect::Environment {
                definition,
                body,
                captures,
                sizes,
                movable,
            } => {
                let captures: Vec<(TypeId, Passing)> =
                    captures.iter().map(|&(t, c)| (t, c.into())).collect();
                let shape = EnvironmentShape {
                    definition: *definition,
                    body: *body,
                    captures: &captures,
                    sizes,
                    movable,
                };
                (
                    Some(
                        layouts
                            .environment(&types, &shape)
                            .map_err(ReplayError::Layout)?,
                    ),
                    None,
                )
            }
            Effect::FlatBuilder(element) => {
                let element = resolve(&derived, *element)?;
                (
                    Some(layouts.flat_builder(element).map_err(ReplayError::Layout)?),
                    None,
                )
            }
            Effect::Request(r) => {
                let environment = r.environment.map(|e| resolve(&derived, e)).transpose()?;
                let request = Request {
                    body: r.body as usize,
                    path: r.path.clone(),
                    actuals: Actuals {
                        binder: r.binder,
                        arguments: r.arguments.clone(),
                        outer: r.outer.clone(),
                    },
                    callable: r.callable,
                    shape: r.shape.into(),
                    environment,
                    finalizer: r.finalizer,
                };
                crate::library::instantiate(layouts, instances, request)
                    .map_err(ReplayError::Instance)?;
                (None, None)
            }
        };
        derived.push(layout);
        abis.push(abi);
    }
    Ok(())
}
