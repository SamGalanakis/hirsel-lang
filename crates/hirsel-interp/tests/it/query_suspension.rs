use crate::support::*;
use hirsel_interp::{
    AsyncTirProvider, NoOperations, Outcome, Value, run_with_runtime, run_with_runtime_async,
};
use hirsel_syntax::{BinaryOp, DefPath};
use hirsel_tir::*;
use hirsel_types::{GenericArgument, ParamId, Primitive, QueryError, TypeKind};
use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Waker},
};

struct Provider {
    definitions: Vec<DefinitionRef>,
    constant: MaterializedRef,
    events: Rc<RefCell<Vec<String>>>,
    suspend: bool,
}
struct ReadGuard(Rc<RefCell<Vec<String>>>);
impl Drop for ReadGuard {
    fn drop(&mut self) {
        self.0.borrow_mut().push("read released".into());
    }
}
impl Provider {
    async fn demand(&self, name: String) {
        self.events.borrow_mut().push(name);
        let _guard = ReadGuard(self.events.clone());
        let first = Cell::new(self.suspend);
        poll_fn(|cx| {
            if first.replace(false) {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
    fn lookup(&self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.definitions
            .iter()
            .find(|d| d.path.path() == path)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
}
impl AsyncTirProvider for Provider {
    async fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.demand(format!("definition {}", path.0.name)).await;
        self.lookup(path)
    }
    async fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        self.demand(format!("constant {}", path.0.name)).await;
        Ok(self.constant.clone())
    }
    async fn constant_instance(
        &mut self,
        path: &DefPath,
        actuals: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        assert!(actuals.is_empty());
        AsyncTirProvider::constant(self, path).await
    }
    async fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
    async fn instance_body(
        &mut self,
        path: &DefPath,
        _: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.demand(format!("instance {}", path.0.name)).await;
        Ok(None)
    }
}
impl TirProvider for Provider {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.lookup(path)
    }
    fn constant(&mut self, _: &DefPath) -> Result<MaterializedRef, QueryError> {
        Ok(self.constant.clone())
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
}
fn provider(suspend: bool) -> Provider {
    let mut b = Builder::new();
    let before = b.int(Primitive::U32, 5);
    let data = b.local(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    let ty = b.body.places[data.0 as usize].ty;
    b.assign(
        data,
        Rvalue::ReadConstant {
            definition: path("DATA"),
            arguments: vec![],
        },
        3,
        CleanupId(0),
    );
    let sum = b.binary(
        BinaryOp::Add,
        before,
        data,
        TypeKind::Primitive(Primitive::U32),
        4,
        CleanupId(0),
    );
    let result = b.local(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("helper"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(sum),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        5,
    );
    let main = Arc::new(b.finish("main", result));
    let mut helper = Builder::new();
    let argument = helper.parameter(TypeKind::Primitive(Primitive::U32));
    let helper = Arc::new(helper.finish("helper", argument));
    Provider {
        definitions: vec![main, helper],
        constant: Arc::new(Materialized {
            root: ValueNode {
                ty,
                kind: ValueKind::Constant(Constant::Integer(7)),
            },
        }),
        events: Rc::new(RefCell::new(vec![])),
        suspend,
    }
}
fn configuration() -> RunConfiguration {
    RunConfiguration {
        entry: path("main"),
        hash_seed: 0,
        inputs: vec![],
    }
}
#[test]
fn pending_queries_preserve_operands_fuel_and_exact_execution_trace() {
    let runtime = runtime();
    let sources = sources();
    let configuration = configuration();
    let mut immediate = provider(false);
    let expected = run_with_runtime(
        &mut immediate,
        &sources,
        &configuration,
        vec![],
        Default::default(),
        &mut NoOperations,
        &runtime,
    )
    .unwrap();
    let mut suspended = provider(true);
    let events = suspended.events.clone();
    let mut operations = NoOperations;
    let mut execution = std::pin::pin!(run_with_runtime_async(
        &mut suspended,
        &sources,
        &configuration,
        vec![],
        Default::default(),
        &mut operations,
        &runtime
    ));
    let mut waits = 0;
    let actual = loop {
        match execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Pending => {
                waits += 1;
                assert!(waits <= 4);
            }
            Poll::Ready(result) => break result.unwrap(),
        }
    };
    assert_eq!(
        waits, 4,
        "entry, constant, instance selection and callee suspend independently"
    );
    assert_eq!(
        actual, expected,
        "suspension must preserve steps, effects, diagnostics and trace bytes"
    );
    assert!(
        matches!(actual.outcome, Outcome::Returned { value: Value::Integer(n), .. } if n.bits() == 12)
    );
    assert_eq!(
        &*events.borrow(),
        &[
            "definition main",
            "read released",
            "constant DATA",
            "read released",
            "instance helper",
            "read released",
            "definition helper",
            "read released",
        ]
    );
}
#[test]
fn dropping_suspended_execution_releases_its_saved_non_send_read_without_restarting() {
    let runtime = runtime();
    let sources = sources();
    let configuration = configuration();
    let mut provider = provider(true);
    let events = provider.events.clone();
    let mut operations = NoOperations;
    let mut execution = Box::pin(run_with_runtime_async(
        &mut provider,
        &sources,
        &configuration,
        vec![],
        Default::default(),
        &mut operations,
        &runtime,
    ));
    assert!(
        execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert_eq!(&*events.borrow(), &["definition main"]);
    std::mem::drop(execution);
    assert_eq!(&*events.borrow(), &["definition main", "read released"]);
}
