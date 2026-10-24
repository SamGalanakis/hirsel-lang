//! Task-local semantic admission, retained across suspension and implicit cleanup.
use hirsel_core_contract::depth::{Activation, Activations};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Debug, Default)]
pub(crate) struct Task(Rc<RefCell<Activations>>);

#[derive(Debug)]
pub(crate) struct Guard {
    task: Task,
    activation: Option<Activation>,
}

impl Task {
    pub(crate) fn admit(&self) -> Option<Guard> {
        self.guard(false)
    }

    pub(crate) fn finalizer(&self) -> Option<Guard> {
        self.guard(true)
    }

    fn guard(&self, finalizer: bool) -> Option<Guard> {
        let mut counts = self.0.borrow_mut();
        let activation = if finalizer {
            counts.admit_finalizer()
        } else {
            counts.admit()
        }?;
        Some(Guard {
            task: self.clone(),
            activation: Some(activation),
        })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(activation) = self.activation.take() {
            self.task.0.borrow_mut().retire(activation);
        }
    }
}
