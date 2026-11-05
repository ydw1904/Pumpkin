use std::cell::RefCell;
use std::sync::Arc;

use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use tracing::error;

use super::World;

/// Default limit for Vanilla for updating neighbors
const MAX_CHAINED_UPDATES: usize = 1_000_000;

#[derive(Clone, Copy)]
enum UpdateKind {
    Neighbor {
        position: BlockPos,
        source_position: BlockPos,
        source_block: BlockId,
        include_fluid: bool,
    },
    Shape {
        position: BlockPos,
        direction: BlockDirection,
        neighbor_position: BlockPos,
        neighbor_state_id: BlockStateId,
        flags: BlockFlags,
    },
}

impl UpdateKind {
    const fn position(&self) -> &BlockPos {
        match self {
            Self::Neighbor { position, .. } | Self::Shape { position, .. } => position,
        }
    }
}

struct QueuedUpdate<W> {
    /// `None` for the world that started the cascade, which outlives it, so most updates don't
    /// touch the world's shared reference count.
    world: Option<W>,
    kind: UpdateKind,
}

struct UpdateStack<W> {
    /// Identifies the world that started the cascade, see [`world_id`].
    root: usize,
    remaining: Vec<QueuedUpdate<W>>,
    staged: Vec<QueuedUpdate<W>>,
    submitted: usize,
    limit: usize,
    gave_up: bool,
}

impl<W: Clone> UpdateStack<W> {
    const fn new(root: usize, limit: usize) -> Self {
        Self {
            root,
            remaining: Vec::new(),
            staged: Vec::new(),
            submitted: 0,
            limit,
            gave_up: false,
        }
    }

    fn stage(&mut self, world: &W, world_id: usize, kinds: impl Iterator<Item = UpdateKind>) {
        for kind in kinds {
            if self.submitted >= self.limit {
                if !self.gave_up {
                    self.gave_up = true;
                    error!(
                        "block updates chained beyond {}, updates starting at {} have been skipped",
                        self.limit,
                        kind.position()
                    );
                }
                return;
            }
            self.submitted += 1;
            self.staged.push(QueuedUpdate {
                world: (world_id != self.root).then(|| world.clone()),
                kind,
            });
        }
    }

    fn promote_staged(&mut self) {
        // pass in newly submitted updates to be run before older siblings
        self.remaining.extend(self.staged.drain(..).rev());
    }
}

// allow each Rayon worker to drain its cascade
thread_local! {
    static PENDING_UPDATES: RefCell<Option<UpdateStack<Arc<World>>>> =
        const { RefCell::new(None) };
}

struct CascadeGuard;

impl Drop for CascadeGuard {
    fn drop(&mut self) {
        drop(PENDING_UPDATES.try_with(|pending| pending.borrow_mut().take()));
    }
}

pub(super) fn update_neighbors_at(
    world: &Arc<World>,
    position: &BlockPos,
    source_block: &Block,
    except: Option<BlockDirection>,
) {
    let source_block = source_block.id;
    let source_position = *position;
    submit(
        world,
        BlockDirection::update_order()
            .into_iter()
            .filter(|direction| except != Some(*direction))
            .map(|direction| UpdateKind::Neighbor {
                position: position.offset(direction.to_offset()),
                source_position,
                source_block,
                include_fluid: true,
            }),
    );
}

pub(super) fn update_neighbor(world: &Arc<World>, position: &BlockPos, source_block: &Block) {
    submit(
        world,
        std::iter::once(UpdateKind::Neighbor {
            position: *position,
            source_position: *position,
            source_block: source_block.id,
            include_fluid: false,
        }),
    );
}

pub(super) fn update_shape(
    world: &Arc<World>,
    position: &BlockPos,
    direction: BlockDirection,
    flags: BlockFlags,
) {
    let neighbor_position = position.offset(direction.to_offset());
    // captures the source state when an update is submitted
    let neighbor_state_id = world.get_block_state_id(&neighbor_position);
    shape_update(
        world,
        position,
        direction,
        &neighbor_position,
        neighbor_state_id,
        flags,
    );
}

pub(super) fn shape_update(
    world: &Arc<World>,
    position: &BlockPos,
    direction: BlockDirection,
    neighbor_position: &BlockPos,
    neighbor_state_id: BlockStateId,
    flags: BlockFlags,
) {
    submit(
        world,
        std::iter::once(UpdateKind::Shape {
            position: *position,
            direction,
            neighbor_position: *neighbor_position,
            neighbor_state_id,
            flags,
        }),
    );
}

fn world_id(world: &Arc<World>) -> usize {
    Arc::as_ptr(world).addr()
}

fn cascade_in_progress() -> bool {
    PENDING_UPDATES.with(|pending| pending.borrow().is_some())
}

fn submit(world: &Arc<World>, updates: impl Iterator<Item = UpdateKind>) {
    let id = world_id(world);
    if cascade_in_progress() {
        PENDING_UPDATES.with(|pending| {
            if let Some(stack) = pending.borrow_mut().as_mut() {
                stack.stage(world, id, updates);
            }
        });
        return;
    }

    let mut stack = UpdateStack::new(id, MAX_CHAINED_UPDATES);
    stack.stage(world, id, updates);
    stack.promote_staged();

    PENDING_UPDATES.with(|pending| {
        *pending.borrow_mut() = Some(stack);
    });

    let _guard = CascadeGuard;
    run_cascade(world);
}

fn run_cascade(root: &Arc<World>) {
    while let Some(QueuedUpdate { world, kind }) = pop_update() {
        execute(world.as_ref().unwrap_or(root), kind);
        PENDING_UPDATES.with(|pending| {
            if let Some(stack) = pending.borrow_mut().as_mut() {
                stack.promote_staged();
            }
        });
    }
}

fn pop_update() -> Option<QueuedUpdate<Arc<World>>> {
    PENDING_UPDATES.with(|pending| {
        pending
            .borrow_mut()
            .as_mut()
            .and_then(|stack| stack.remaining.pop())
    })
}

fn execute(world: &Arc<World>, kind: UpdateKind) {
    match kind {
        UpdateKind::Neighbor {
            position,
            source_position,
            source_block,
            include_fluid,
        } => {
            world.execute_neighbor_update(
                &position,
                &source_position,
                source_block.to_block(),
                include_fluid,
            );
        }
        UpdateKind::Shape {
            position,
            direction,
            neighbor_position,
            neighbor_state_id,
            flags,
        } => {
            world.execute_shape_update(
                &position,
                direction,
                &neighbor_position,
                neighbor_state_id,
                flags,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CascadeGuard, MAX_CHAINED_UPDATES, PENDING_UPDATES, UpdateKind, UpdateStack,
        cascade_in_progress,
    };
    use pumpkin_data::{BlockDirection, BlockId};
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::math::vector3::Vector3;
    use pumpkin_world::world::BlockFlags;

    const OVERWORLD: &str = "overworld";
    const NETHER: &str = "nether";
    const OVERWORLD_ID: usize = 1;
    const NETHER_ID: usize = 2;

    fn neighbor_at(x: i32) -> UpdateKind {
        UpdateKind::Neighbor {
            position: BlockPos(Vector3::new(x, 0, 0)),
            source_position: BlockPos(Vector3::new(x, 0, 0)),
            source_block: BlockId::AIR,
            include_fluid: true,
        }
    }

    fn remaining_positions(stack: &UpdateStack<&'static str>) -> Vec<i32> {
        stack
            .remaining
            .iter()
            .map(|update| update.kind.position().0.x)
            .collect()
    }

    fn remaining_worlds(stack: &UpdateStack<&'static str>) -> Vec<&'static str> {
        stack
            .remaining
            .iter()
            .map(|update| update.world.unwrap_or(OVERWORLD))
            .collect()
    }

    #[test]
    fn staged_updates_are_popped_in_submission_order() {
        let mut stack = UpdateStack::new(OVERWORLD_ID, MAX_CHAINED_UPDATES);
        stack.stage(
            &OVERWORLD,
            OVERWORLD_ID,
            [neighbor_at(1), neighbor_at(2), neighbor_at(3)].into_iter(),
        );
        stack.promote_staged();

        assert_eq!(remaining_positions(&stack), vec![3, 2, 1]);
        assert_eq!(
            stack
                .remaining
                .pop()
                .map(|update| update.kind.position().0.x),
            Some(1)
        );
    }

    #[test]
    fn updates_staged_while_running_are_handled_before_the_rest() {
        let mut stack = UpdateStack::new(OVERWORLD_ID, MAX_CHAINED_UPDATES);
        stack.stage(
            &OVERWORLD,
            OVERWORLD_ID,
            [neighbor_at(1), neighbor_at(2)].into_iter(),
        );
        stack.promote_staged();
        stack.remaining.pop();

        stack.stage(
            &OVERWORLD,
            OVERWORLD_ID,
            [neighbor_at(10), neighbor_at(11)].into_iter(),
        );
        stack.promote_staged();

        assert_eq!(remaining_positions(&stack), vec![2, 11, 10]);
    }

    #[test]
    fn updates_stay_bound_to_the_world_that_staged_them() {
        let mut stack = UpdateStack::new(OVERWORLD_ID, MAX_CHAINED_UPDATES);
        stack.stage(
            &OVERWORLD,
            OVERWORLD_ID,
            [neighbor_at(1), neighbor_at(2)].into_iter(),
        );
        stack.promote_staged();

        assert_eq!(
            stack.remaining.pop().map(|update| (
                update.world.unwrap_or(OVERWORLD),
                update.kind.position().0.x
            )),
            Some((OVERWORLD, 1)),
        );

        stack.stage(&NETHER, NETHER_ID, std::iter::once(neighbor_at(10)));
        stack.promote_staged();

        assert_eq!(remaining_positions(&stack), vec![2, 10]);
        assert_eq!(remaining_worlds(&stack), vec![OVERWORLD, NETHER]);
    }

    #[test]
    fn updates_chained_past_the_budget_are_skipped() {
        let mut stack = UpdateStack::new(OVERWORLD_ID, 2);
        stack.stage(
            &OVERWORLD,
            OVERWORLD_ID,
            [neighbor_at(1), neighbor_at(2), neighbor_at(3)].into_iter(),
        );
        stack.stage(
            &OVERWORLD,
            OVERWORLD_ID,
            std::iter::once(UpdateKind::Shape {
                position: BlockPos(Vector3::new(4, 0, 0)),
                direction: BlockDirection::Up,
                neighbor_position: BlockPos(Vector3::new(4, 1, 0)),
                neighbor_state_id: pumpkin_data::BlockStateId::AIR,
                flags: BlockFlags::NOTIFY_ALL,
            }),
        );
        stack.promote_staged();

        assert!(stack.gave_up);
        assert_eq!(remaining_positions(&stack), vec![2, 1]);
    }

    #[test]
    #[expect(clippy::panic, reason = "Checks that unwinding resets the cascade")]
    fn panic_does_not_leave_a_cascade_running() {
        let result = std::panic::catch_unwind(|| {
            PENDING_UPDATES.with(|pending| {
                *pending.borrow_mut() = Some(UpdateStack::new(OVERWORLD_ID, MAX_CHAINED_UPDATES));
            });
            let _guard = CascadeGuard;
            panic!("block callback panicked");
        });
        assert!(result.is_err());
        assert!(!cascade_in_progress());
    }
}
