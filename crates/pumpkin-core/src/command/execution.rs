//! Vanilla `ExecutionContext`: function calls run from one flat command queue per top-level
//! command, so a function calling itself never grows the stack and stops at
//! `max_command_sequence_length`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use tracing::{error, info};

use crate::command::context::command_source::ResultValueTaker;
use crate::command::context::command_source::{CommandSource, Frame, FrameControl};
use crate::server::Server;

/// Vanilla `ExecutionContext.MAX_QUEUE_DEPTH`.
const MAX_QUEUE_DEPTH: usize = 10_000_000;

/// How deeply `execute if function` may nest. Vanilla continues the command after the functions
/// ran from the queue; Pumpkin runs them before the chain goes on, which uses the stack.
const MAX_NESTED_CALLS: usize = 32;

thread_local! {
    /// Vanilla `Commands.CURRENT_EXECUTION_CONTEXT`.
    static CURRENT: RefCell<Option<Rc<ExecutionContext>>> = const { RefCell::new(None) };
}

/// Vanilla `CommandQueueEntry`.
pub struct Entry {
    pub frame: Frame,
    pub action: Action,
}

/// What a queue entry does.
pub enum Action {
    /// Vanilla `CallFunction`: runs a function's commands in a new frame.
    CallFunction {
        source: Box<CommandSource>,
        lines: Arc<[String]>,
        callback: ResultValueTaker,
        /// A `return` in the function also returns from the calling frame.
        return_parent: bool,
    },
    /// Vanilla `ContinuationTask`: queues one command of a function, then the rest.
    Continue {
        source: Arc<CommandSource>,
        lines: Arc<[String]>,
        index: usize,
    },
    /// Vanilla `BuildContexts.Unbound`: one command of a function.
    Command {
        source: Arc<CommandSource>,
        lines: Arc<[String]>,
        index: usize,
    },
    /// Vanilla `FallthroughTask`: the frame returns failure and ends.
    Fallthrough,
    Task(Box<dyn FnOnce()>),
}

pub struct ExecutionContext {
    command_limit: i64,
    state: RefCell<State>,
}

struct State {
    command_quota: i64,
    queue_overflow: bool,
    queue: VecDeque<Entry>,
    new_top_commands: Vec<Entry>,
    current_frame_depth: usize,
    nested_calls: usize,
}

/// Clears the thread's current context when the top-level command ends, even by a panic.
struct CurrentGuard;

impl Drop for CurrentGuard {
    fn drop(&mut self) {
        let context = CURRENT.with(|current| current.borrow_mut().take());
        if let Some(context) = context {
            // Entries hold frames and sources; drop them outside the thread-local borrow.
            let mut state = context.state.borrow_mut();
            state.queue.clear();
            state.new_top_commands.clear();
        }
    }
}

impl ExecutionContext {
    const fn new(command_limit: i64) -> Self {
        Self {
            command_limit,
            state: RefCell::new(State {
                command_quota: command_limit,
                queue_overflow: false,
                queue: VecDeque::new(),
                new_top_commands: Vec::new(),
                current_frame_depth: 0,
                nested_calls: 0,
            }),
        }
    }

    /// The context of the command running on this thread.
    #[must_use]
    pub fn current() -> Option<Rc<Self>> {
        CURRENT.with(|current| current.borrow().clone())
    }

    /// Vanilla `Commands.executeCommandInContext`: runs `config` in the current context, or in a
    /// new one whose queue is run afterwards. Returns how much of the command limit a new
    /// context used.
    pub fn execute_in_context(server: &Server, config: impl FnOnce(&Self)) -> usize {
        if let Some(context) = Self::current() {
            config(&context);
            return 0;
        }
        let chain_limit = server
            .level_info
            .load()
            .game_rules
            .max_command_sequence_length
            .max(1);
        let context = Rc::new(Self::new(chain_limit));
        CURRENT.with(|current| *current.borrow_mut() = Some(context.clone()));
        let _guard = CurrentGuard;
        config(&context);
        context.run_command_queue();
        usize::try_from(chain_limit - context.state.borrow().command_quota).unwrap_or(0)
    }

    /// Vanilla `ExecutionContext.createTopFrame`.
    #[must_use]
    pub fn top_frame(&self, return_value: ResultValueTaker) -> Frame {
        let depth = self.state.borrow().current_frame_depth;
        if depth == 0 {
            Frame {
                depth: 0,
                return_value,
                control: FrameControl::ClearQueue,
            }
        } else {
            Frame {
                depth: depth + 1,
                return_value,
                control: FrameControl::DiscardAtDepth(depth + 1),
            }
        }
    }

    /// The depth of the frame the running command belongs to.
    #[must_use]
    pub fn current_frame_depth(&self) -> usize {
        self.state.borrow().current_frame_depth
    }

    /// Vanilla `ExecutionContext.queueNext`: the entry runs after the running one, before
    /// anything queued earlier.
    pub fn queue_next(&self, entry: Entry) {
        let mut state = self.state.borrow_mut();
        if state.new_top_commands.len() + state.queue.len() > MAX_QUEUE_DEPTH {
            state.queue_overflow = true;
            state.new_top_commands.clear();
            state.queue.clear();
        }
        if !state.queue_overflow {
            state.new_top_commands.push(entry);
        }
    }

    /// Vanilla `ExecutionContext.incrementCost`.
    pub fn increment_cost(&self) {
        self.state.borrow_mut().command_quota -= 1;
    }

    #[must_use]
    pub fn quota_exhausted(&self) -> bool {
        self.state.borrow().command_quota <= 0
    }

    /// Vanilla `Frame.discard` through its `FrameControl`.
    pub fn discard(control: FrameControl) {
        let Some(context) = Self::current() else {
            return;
        };
        let removed: Vec<Entry> = {
            let mut state = context.state.borrow_mut();
            match control {
                FrameControl::ClearQueue => state.queue.drain(..).collect(),
                FrameControl::DiscardAtDepth(depth) => {
                    let count = state
                        .queue
                        .iter()
                        .take_while(|entry| entry.frame.depth >= depth)
                        .count();
                    state.queue.drain(..count).collect()
                }
            }
        };
        drop(removed);
    }

    /// Vanilla `ExecutionContext.runCommandQueue`.
    fn run_command_queue(&self) {
        self.push_new_commands();
        loop {
            if self.quota_exhausted() {
                info!(
                    "Command execution stopped due to limit (executed {} commands)",
                    self.command_limit
                );
                break;
            }
            let Some(entry) = self.next_entry(None) else {
                return;
            };
            self.execute(entry);
            if self.state.borrow().queue_overflow {
                error!(
                    "Command execution stopped due to command queue overflow (max {MAX_QUEUE_DEPTH})"
                );
                break;
            }
            self.push_new_commands();
        }
        self.state.borrow_mut().current_frame_depth = 0;
    }

    /// Runs what `config` queues in a frame below the running command, before the command goes
    /// on (vanilla `IsolatedCall`, whose result the rest of the command waits for).
    pub fn run_nested(&self, config: impl FnOnce(&Self, Frame), return_value: ResultValueTaker) {
        let (outer_new_top, depth) = {
            let mut state = self.state.borrow_mut();
            if state.nested_calls >= MAX_NESTED_CALLS {
                state.command_quota = 0;
                return;
            }
            state.nested_calls += 1;
            (
                std::mem::take(&mut state.new_top_commands),
                state.current_frame_depth,
            )
        };
        let frame = Frame {
            depth: depth + 1,
            return_value,
            control: FrameControl::DiscardAtDepth(depth + 1),
        };
        config(self, frame);
        self.push_new_commands();
        while !self.quota_exhausted() && !self.state.borrow().queue_overflow {
            let Some(entry) = self.next_entry(Some(depth)) else {
                break;
            };
            self.execute(entry);
            self.push_new_commands();
        }
        let mut state = self.state.borrow_mut();
        state.nested_calls -= 1;
        state.current_frame_depth = depth;
        state.new_top_commands = outer_new_top;
    }

    /// The next entry to run, if it's deeper than `above`.
    fn next_entry(&self, above: Option<usize>) -> Option<Entry> {
        let mut state = self.state.borrow_mut();
        if let Some(depth) = above
            && state
                .queue
                .front()
                .is_none_or(|entry| entry.frame.depth <= depth)
        {
            return None;
        }
        let entry = state.queue.pop_front()?;
        state.current_frame_depth = entry.frame.depth;
        Some(entry)
    }

    fn push_new_commands(&self) {
        let mut state = self.state.borrow_mut();
        let new_top = std::mem::take(&mut state.new_top_commands);
        for entry in new_top.into_iter().rev() {
            state.queue.push_front(entry);
        }
    }

    /// Vanilla `CommandQueueEntry.execute`.
    fn execute(&self, entry: Entry) {
        let frame = entry.frame;
        match entry.action {
            Action::CallFunction {
                mut source,
                lines,
                callback,
                return_parent,
            } => {
                self.increment_cost();
                let depth = frame.depth + 1;
                let control = if return_parent {
                    frame.control
                } else {
                    FrameControl::DiscardAtDepth(depth)
                };
                let frame = Frame {
                    depth,
                    return_value: callback,
                    control,
                };
                source.frame = frame.clone();
                source.returning = false;
                if !lines.is_empty() {
                    self.queue_next(Entry {
                        frame,
                        action: Action::Continue {
                            source: Arc::new(*source),
                            lines,
                            index: 0,
                        },
                    });
                }
            }
            Action::Continue {
                source,
                lines,
                index,
            } => {
                let more = index + 1 < lines.len();
                self.queue_next(Entry {
                    frame: frame.clone(),
                    action: Action::Command {
                        source: source.clone(),
                        lines: lines.clone(),
                        index,
                    },
                });
                if more {
                    self.queue_next(Entry {
                        frame,
                        action: Action::Continue {
                            source,
                            lines,
                            index: index + 1,
                        },
                    });
                }
            }
            Action::Command {
                source,
                lines,
                index,
            } => {
                let dispatcher = source.server().command_dispatcher.load();
                dispatcher.handle_command(&source, &lines[index]);
            }
            Action::Fallthrough => {
                frame.return_failure();
                frame.discard();
            }
            Action::Task(task) => task(),
        }
    }
}
