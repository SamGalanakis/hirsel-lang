//! Closed CT-5 selection before front publication. This is the existing
//! select_generated_instances worklist with explicit admitted summaries instead
//! of scalar front_summary demands. Missing bodies are returned to the caller;
//! neither discovery nor syntax registration checks or executes a body.
use super::*;
use crate::runtime::MemoryReservation;

#[derive(Clone, Debug)]
pub struct FrontInstanceRequest {
    pub declaration: DefPath,
    pub actuals: hirsel_types::Actuals,
    pub site: Span,
}
/// Syntax/identity admission, independent of private generator execution.
/// The engine's source snapshot also retains this grant while it owns syntax.
pub struct FrontInstanceSyntax {
    definition: DefPath,
    retained: u64,
    _allowance: Arc<MemoryReservation>,
}
impl FrontInstanceSyntax {
    pub fn definition(&self) -> &DefPath {
        &self.definition
    }
    pub fn retained_bytes(&self) -> u64 {
        self.retained
    }
}
/// One bounded discovery round. Requests are canonical declaration/argument
/// order; missing body paths are sorted. A caller registers requests, executes
/// their generators through FrontRegistrationQueries, accepts their outputs,
/// and supplies the next admitted front view until pending_bodies is empty.
pub struct FrontInstanceDiscovery {
    requests: Vec<FrontInstanceRequest>,
    pending_bodies: Vec<DefPath>,
    retained: u64,
    _allowance: MemoryReservation,
}
impl FrontInstanceDiscovery {
    pub fn requests(&self) -> &[FrontInstanceRequest] {
        &self.requests
    }
    pub fn pending_bodies(&self) -> &[DefPath] {
        &self.pending_bodies
    }
    pub fn retained_bytes(&self) -> u64 {
        self.retained
    }
}
fn refusal(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "generated instance registration".into(),
        message: message.into(),
    })
}
impl QueryEngine {
    /// Register the existing canonical identity and substituted syntax only.
    /// A failed admission rolls source/namespace changes back; typed diagnostic
    /// sources remain owned by the failure. No Modes/Lower/execution demand.
    pub fn register_front_instance(
        &mut self,
        request: &FrontInstanceRequest,
        allowance: MemoryReservation,
    ) -> Result<Arc<FrontInstanceSyntax>, FrontRegistrationFailure> {
        let allowance = Arc::new(allowance);
        let previous_sources = self.sources.clone();
        let source_before = self.sources.retained_payload_bytes();
        let mut installed = None;
        let result = (|| {
            self.materialize(&request.declaration)?;
            let syntax = self.snapshot(&request.declaration)?;
            let ItemKind::Function { signature, .. } = &syntax.item.kind else {
                return Err(refusal("expected a generic generated function"));
            };
            if !instance_bodied(&syntax)
                || signature.generics.len() != request.actuals.arguments.len()
                || !signature
                    .generics
                    .iter()
                    .zip(&request.actuals.arguments)
                    .all(|(p, a)| {
                        matches!(
                            (&p.kind, a),
                            (
                                hirsel_syntax::GenericKind::Type(_),
                                hirsel_types::GenericArgument::Type(_)
                            ) | (
                                hirsel_syntax::GenericKind::Size,
                                hirsel_types::GenericArgument::Size(hirsel_types::Size::Constant(
                                    _
                                ))
                            )
                        )
                    })
            {
                return Err(refusal(
                    "closed arguments do not match the declaring header",
                ));
            }
            let key = hirsel_tir::InstanceBodies::key(&self.types, &request.actuals.arguments)
                .ok_or_else(|| refusal("instance selected at open arguments"))?;
            let expected = instance_path(&request.declaration, key);
            if !self
                .instance_bodies
                .instances
                .get(&DefId::intern(&request.declaration))
                .is_some_and(|instances| instances.contains_key(&key))
            {
                installed = Some((expected, key));
            }
            let (definition, fresh) = self.register_generated_instance(
                &request.declaration,
                &request.actuals,
                request.site,
            )?;
            let mut weight = crate::NativeMetadataWeight::default();
            weight.add(size_of::<FrontInstanceSyntax>() + 2 * size_of::<usize>());
            weight.add(size_of::<MemoryReservation>() + 2 * size_of::<usize>());
            weight.path(&definition);
            if fresh {
                weight.add(
                    self.sources
                        .retained_payload_bytes()
                        .saturating_sub(source_before)
                        .try_into()
                        .map_err(|_| refusal("instance source size overflow"))?,
                );
                weight.source_snapshot(&self.sources);
                weight.tree::<hirsel_span::Anchor, (FileId, u32)>(self.sources.anchors().count());
                weight.add(
                    crate::memo::retained::retained_syntax_bytes(&*self.snapshot(&definition)?)
                        .try_into()
                        .map_err(|_| refusal("instance syntax size overflow"))?,
                );
                weight.tree::<DefPath, Vec<Arc<ItemSyntax>>>(1);
                weight.path(&definition);
                weight.add(size_of::<Arc<ItemSyntax>>());
                weight.tree::<hirsel_types::GeneratedActualKey, hirsel_tir::InstanceBody>(1);
                weight.add(size_of_val(request.actuals.arguments.as_slice()));
                for spelling in key.spellings() {
                    weight.add(spelling.len());
                }
                weight.tree::<DefId, ()>(1);
            }
            let retained = weight.bytes();
            if retained > allowance.bytes() {
                return Err(refusal("instance syntax exceeds its reservation"));
            }
            if fresh {
                self.sources = self.sources.clone().retain_owner(allowance.clone());
            }
            Ok(Arc::new(FrontInstanceSyntax {
                definition,
                retained,
                _allowance: allowance.clone(),
            }))
        })();
        match result {
            Ok(value) => Ok(value),
            Err(mut error) => {
                let mut weight = crate::NativeMetadataWeight::default();
                weight.add(size_of::<MemoryReservation>() + 2 * size_of::<usize>());
                weight.source_snapshot(&self.sources);
                weight.add(
                    crate::memo::retained::retained_query_error_bytes(&error)
                        .try_into()
                        .unwrap_or(usize::MAX),
                );
                let sources = if matches!(error, QueryError::Diagnostics(_))
                    && weight.bytes() <= allowance.bytes()
                {
                    self.sources.clone().retain_owner(allowance.clone())
                } else {
                    if matches!(error, QueryError::Diagnostics(_)) {
                        error = refusal("instance diagnostics exceed their reservation");
                    }
                    previous_sources.clone()
                };
                self.sources = previous_sources;
                if let Some((path, key)) = installed {
                    self.items.remove(&path);
                    let id = DefId::intern(&request.declaration);
                    if let Some(instances) = self.instance_bodies.instances.get_mut(&id) {
                        instances.remove(&key);
                        if instances.is_empty() {
                            self.instance_bodies.instances.remove(&id);
                            self.instance_bodies.declarations.remove(&id);
                        }
                    }
                    self.note_inputs([path]);
                    self.retire_snapshot();
                }
                Err(FrontRegistrationFailure {
                    error,
                    sources,
                    _allowance: allowance,
                })
            }
        }
    }

    /// Declaring text is an ordinary read, not a second identity scheme. The
    /// worker also records instance text and all its own execution reads.
    pub(crate) fn front_instance_reads(&mut self, path: &DefPath) -> Result<Reads, QueryError> {
        let selected =
            self.instance_bodies
                .instances
                .iter()
                .find_map(|(declaration, instances)| {
                    instances
                        .values()
                        .find(|i| i.definition.path() == path)
                        .map(|instance| (*declaration, instance.arguments.clone()))
                });
        let Some((declaration, arguments)) = selected else {
            return Ok(Reads::empty());
        };
        let mut reads = ReadsBuilder::new();
        reads.record(
            ReadKey::ExecutedBody(declaration),
            memos::optional(self.implementation_hash(declaration.path())),
        );
        // Closed nominal actuals commit to their headers even if this generator
        // does not reflect their shape. Traverse the existing type graph.
        let mut pending: Vec<_> = arguments
            .iter()
            .filter_map(|a| match a {
                hirsel_types::GenericArgument::Type(ty) => Some(*ty),
                _ => None,
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        let snapshot = self.signature_inputs();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self
                .types
                .get(ty)
                .ok_or_else(|| refusal("missing closed argument type"))?;
            if let hirsel_types::TypeKind::Named { definition, .. } = &row.kind {
                let fingerprint = snapshot
                    .signature(*definition)
                    .and_then(Result::ok)
                    .map(|s| s.fingerprint);
                reads.record(
                    ReadKey::Signature(*definition),
                    memos::signature_value(fingerprint),
                );
            }
            pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
        }
        reads
            .finish()
            .map_err(|_| refusal("inconsistent closed argument reads"))
    }

    /// Walk only caller-supplied admitted products. Missing summaries suspend
    /// discovery, so generated-to-generated selection uses explicit rounds.
    /// Bounds and closing semantics match select_generated_instances.
    pub fn discover_front_instances(
        &mut self,
        roots: &[DefPath],
        fronts: &[Arc<FrontBodyQueries>],
        allowance: MemoryReservation,
    ) -> Result<FrontInstanceDiscovery, QueryError> {
        let mut pending: Vec<_> = roots
            .iter()
            .rev()
            .cloned()
            .map(|p| (p, BTreeMap::new()))
            .collect();
        let mut seen = BTreeSet::new();
        let mut requests = BTreeMap::new();
        let mut missing = BTreeSet::new();
        while let Some((path, bindings)) = pending.pop() {
            let binding_key: Vec<_> = bindings
                .iter()
                .map(
                    |(site, actuals): (&hirsel_types::SiteRef, &hirsel_types::Actuals)| {
                        (
                            *site,
                            hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments),
                        )
                    },
                )
                .collect();
            if !seen.insert((path.clone(), binding_key)) {
                continue;
            }
            if seen.len() > 65536 {
                return Err(refusal("instance selection expansion limit"));
            }
            if !self.declared(&path) || self.instance_bodied(&path) {
                continue;
            }
            let mut product = None;
            for front in fronts {
                if let Some(output) = front.instance_discovery_summary(self, &path)? {
                    product = Some(output);
                    break;
                }
            }
            let Some(product) = product else {
                missing.insert(path);
                continue;
            };
            let summary = &product.summary.summary;
            if bindings.is_empty() && summary.frames[0].generic {
                continue;
            }
            for call in summary
                .calls
                .iter()
                .filter(|call| call.target_frame.is_none())
            {
                let callee = call.callee.path();
                if !self.declared(callee) {
                    continue;
                }
                let actuals = hirsel_types::close_arguments(
                    &self.types,
                    &self.memo,
                    &call.arguments,
                    &bindings,
                )
                .map_err(|e| refusal(format!("closing {callee} arguments: {e:?}")))?;
                let key = hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments);
                if self.instance_bodied(callee) {
                    if let Some(key) = key {
                        requests.entry((callee.clone(), key)).or_insert_with(|| {
                            FrontInstanceRequest {
                                declaration: callee.clone(),
                                actuals,
                                site: call.site,
                            }
                        });
                        let selected: BTreeSet<_> = requests
                            .keys()
                            .filter(|(d, _)| d == callee)
                            .map(|(_, k)| *k)
                            .chain(
                                self.instance_bodies
                                    .instances
                                    .get(&call.callee)
                                    .into_iter()
                                    .flat_map(|i| i.keys().copied()),
                            )
                            .collect();
                        if selected.len() > MAX_GENERATED_INSTANCES {
                            let syntax = self.snapshot(callee)?;
                            let body = generator(&syntax).ok_or(QueryError::NotImplemented)?;
                            let site = syntax.tree.expressions[body.0 as usize].origin.span;
                            let mut d = Diagnostic::error(
                                const { hirsel_diag::RuleId::of("CT-1") },
                                site,
                                format!(
                                    "a generic generated declaration selects at most {MAX_GENERATED_INSTANCES} instances"
                                ),
                            );
                            d.secondary.push(hirsel_diag::Label {
                                span: call.site,
                                message: format!("selects instance {}", key.instance_name(callee)),
                            });
                            return Err(QueryError::Diagnostics(vec![d]));
                        }
                        let instance = instance_path(callee, key);
                        if self.declared(&instance) {
                            pending.push((instance, BTreeMap::new()));
                        } else {
                            missing.insert(instance);
                        }
                    }
                } else if actuals.arguments.is_empty() {
                    pending.push((callee.clone(), BTreeMap::new()));
                } else if key.is_some() {
                    pending.push((callee.clone(), BTreeMap::from([(call.binder, actuals)])));
                }
            }
        }
        let requests: Vec<_> = requests.into_values().collect();
        let pending_bodies: Vec<_> = missing.into_iter().collect();
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<FrontInstanceDiscovery>());
        weight.vec(&requests, |w, r| {
            w.path(&r.declaration);
            w.flat_vec(&r.actuals.arguments);
        });
        weight.vec(&pending_bodies, |w, path| w.path(path));
        let retained = weight.bytes();
        if retained > allowance.bytes() {
            return Err(refusal("instance discovery exceeds its reservation"));
        }
        Ok(FrontInstanceDiscovery {
            requests,
            pending_bodies,
            retained,
            _allowance: allowance,
        })
    }
}
