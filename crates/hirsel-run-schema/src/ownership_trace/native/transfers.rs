//! Checked caller custody is independent of the runtime's FIFO receipt.
use super::{ChannelBinding, ChannelDirection, Context, Error, Owner, Storage, Tracker};

pub(super) struct Intent {
    pub marker: u32,
    pub binding: ChannelBinding,
    pub owner: Owner,
    pub context: Context,
    pub optional: bool,
}

fn occupied(tracker: &Tracker, root: &Owner) -> bool {
    tracker.inventory().owners.keys().any(|owner| {
        owner.task == root.task
            && owner.storage == root.storage
            && owner.path.starts_with(&root.path)
    })
}

impl Intent {
    pub(super) fn commit(
        mut self,
        tracker: &mut Tracker,
        queue: u64,
        item: u64,
        status: u32,
        operation: u64,
    ) -> Result<(), Error> {
        if status != 0 {
            if self.binding.direction == ChannelDirection::Receive && occupied(tracker, &self.owner)
            {
                return Err(Error::InvalidMarker(self.marker));
            }
            return Ok(());
        }
        let object = Owner {
            task: None,
            storage: Storage::Object {
                name: "native.channel".into(),
                id: queue,
                item,
            },
            path: Vec::new(),
        };
        let (from, to) = match self.binding.direction {
            ChannelDirection::Send => (&self.owner, &object),
            ChannelDirection::Receive => (&object, &self.owner),
        };
        if occupied(tracker, to) {
            return Err(Error::InvalidMarker(self.marker));
        }
        if self.optional && !occupied(tracker, from) {
            return Ok(());
        }
        self.context.operation = operation;
        tracker
            .move_tree(&self.context, from, to)
            .map_err(Error::Consistency)?;
        Ok(())
    }
}
