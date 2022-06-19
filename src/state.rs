use crate::event::UpdateEvent;
use crate::sequence::sequence;
use crate::state::State::*;
use crate::status::Status::*;
use crate::when_all::when_all;
use crate::{Behavior, Event, Status, UpdateArgs};
// use serde_derive::{Deserialize, Serialize};

/// The action is still running.
pub const RUNNING: (Status, f64) = (Running, 0.0);

/// The arguments in the action callback.
pub struct ActionArgs<'a, E: 'a, A: 'a, S: 'a> {
    /// The event.
    pub event: &'a E,
    /// The remaining delta time.
    pub dt: f64,
    /// The action running.
    pub action: &'a A,
    /// The state of the running action, if any.
    pub state: &'a mut Option<S>,
    // data
    // pub data: Option<&'a mut D>,
}

/// Keeps track of a behavior.
#[derive(Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub enum State<A, S> {
    /// Executes an action.
    ActionState(A, Option<S>),
    /// Converts `Success` into `Failure` and vice versa.
    FailState(Box<State<A, S>>),
    /// Ignores failures and always return `Success`.
    AlwaysSucceedState(Box<State<A, S>>),
    /// Keeps track of waiting for a period of time before continuing.
    ///
    /// f64: Total time in seconds to wait
    ///
    /// f64: Time elapsed in seconds
    WaitState(f64, f64),
    /// Waits forever.
    WaitForeverState,
    /// Keeps track of an `If` behavior.
    /// If status is `Running`, then it evaluates the condition.
    /// If status is `Success`, then it evaluates the success behavior.
    /// If status is `Failure`, then it evaluates the failure behavior.
    IfState(Box<Behavior<A>>, Box<Behavior<A>>, Status, Box<State<A, S>>),
    /// Keeps track of a `Select` behavior.
    SelectState(Vec<Behavior<A>>, usize, Box<State<A, S>>),
    /// Keeps track of an `Sequence` behavior.
    SequenceState(Vec<Behavior<A>>, usize, Box<State<A, S>>),
    /// Keeps track of a `While` behavior.
    WhileState(Box<State<A, S>>, Vec<Behavior<A>>, usize, Box<State<A, S>>),
    /// Keeps track of a `WhenAll` behavior.
    WhenAllState(Vec<Option<State<A, S>>>),
    /// Keeps track of a `WhenAny` behavior.
    WhenAnyState(Vec<Option<State<A, S>>>),
    /// Keeps track of an `After` behavior.
    AfterState(usize, Vec<State<A, S>>),
}

impl<A: Clone, S> State<A, S> {
    /// Creates a state from a behavior.
    pub fn new(behavior: Behavior<A>) -> Self {
        match behavior {
            Behavior::Action(action) => State::ActionState(action, None),
            Behavior::Fail(ev) => State::FailState(Box::new(State::new(*ev))),
            Behavior::AlwaysSucceed(ev) => State::AlwaysSucceedState(Box::new(State::new(*ev))),
            Behavior::Wait(dt) => State::WaitState(dt, 0.0),
            Behavior::WaitForever => State::WaitForeverState,
            Behavior::If(condition, success, failure) => {
                let state = State::new(*condition);
                State::IfState(success, failure, Status::Running, Box::new(state))
            }
            Behavior::Select(sel) => {
                let state = State::new(sel[0].clone());
                State::SelectState(sel, 0, Box::new(state))
            }
            Behavior::Sequence(seq) => {
                let state = State::new(seq[0].clone());
                State::SequenceState(seq, 0, Box::new(state))
            }
            Behavior::While(ev, rep) => {
                let state = State::new(rep[0].clone());
                State::WhileState(Box::new(State::new(*ev)), rep, 0, Box::new(state))
            }
            Behavior::WhenAll(all) => State::WhenAllState(all.into_iter().map(|ev| Some(State::new(ev))).collect()),
            Behavior::WhenAny(all) => State::WhenAnyState(all.into_iter().map(|ev| Some(State::new(ev))).collect()),
            Behavior::After(seq) => State::AfterState(0, seq.into_iter().map(State::new).collect()),
        }
    }

    /// A signal called "tick" is sent to the root
    /// of the tree and propagates through the tree
    /// until it reaches a leaf / Action node.
    ///
    /// A TreeNode that receives a tick signal executes it's callback.
    /// This callback must return either SUCCESS, FAILURE or RUNNING
    pub fn tick<F>(&mut self, dt: f64, mut block: F) -> ()
    where
        F: FnMut(ActionArgs<'_, Event, A, S>) -> (Status, f64),
    {
        let e: Event = UpdateArgs { dt }.into();
        self.event(&e, &mut block);
    }

    /// Updates the cursor that tracks an event.
    ///
    /// The action need to return status and remaining delta time.
    /// Returns status and the remaining delta time.
    ///
    /// Passes event, delta time in seconds, action and state to closure.
    /// The closure should return a status and remaining delta time.
    pub fn event<E, F>(&mut self, e: &E, f: &mut F) -> (Status, f64)
    where
        E: UpdateEvent,
        F: FnMut(ActionArgs<E, A, S>) -> (Status, f64),
    {
        let upd = e.update(|args| Some(args.dt)).unwrap_or(None);
        match (upd, self) {
            (_, &mut ActionState(ref action, ref mut state)) => {
                // Execute action.
                f(ActionArgs {
                    event: e,
                    dt: upd.unwrap_or(0.0),
                    action,
                    state,
                })
            }
            (_, &mut FailState(ref mut cur)) => match cur.event(e, f) {
                (Running, dt) => (Running, dt),
                (Failure, dt) => (Success, dt),
                (Success, dt) => (Failure, dt),
            },
            (_, &mut AlwaysSucceedState(ref mut cur)) => match cur.event(e, f) {
                (Running, dt) => (Running, dt),
                (_, dt) => (Success, dt),
            },
            (Some(dt), &mut WaitState(wait_t, ref mut t)) => {
                if *t + dt >= wait_t {
                    let remaining_dt = *t + dt - wait_t;
                    *t = wait_t;
                    (Success, remaining_dt)
                } else {
                    *t += dt;
                    RUNNING
                }
            }
            (_, &mut IfState(ref success, ref failure, ref mut status, ref mut state)) => {
                let mut remaining_dt = upd.unwrap_or(0.0);
                let remaining_e;
                // Run in a loop to evaluate success or failure with
                // remaining delta time after condition.
                loop {
                    *status = match *status {
                        Running => match state.event(e, f) {
                            (Running, dt) => {
                                return (Running, dt);
                            }
                            (Success, dt) => {
                                **state = State::new((**success).clone());
                                remaining_dt = dt;
                                Success
                            }
                            (Failure, dt) => {
                                **state = State::new((**failure).clone());
                                remaining_dt = dt;
                                Failure
                            }
                        },
                        _ => {
                            return state.event(
                                match upd {
                                    Some(_) => {
                                        remaining_e = UpdateEvent::from_dt(remaining_dt, e).unwrap();
                                        &remaining_e
                                    }
                                    _ => e,
                                },
                                f,
                            );
                        }
                    }
                }
            }
            (_, &mut SelectState(ref seq, ref mut i, ref mut cursor)) => {
                let select = true;
                sequence(select, upd, seq, i, cursor, e, f)
            }
            (_, &mut SequenceState(ref seq, ref mut i, ref mut cursor)) => {
                let select = false;
                sequence(select, upd, seq, i, cursor, e, f)
            }
            (_, &mut WhileState(ref mut ev_cursor, ref rep, ref mut i, ref mut cursor)) => {
                // If the event terminates, do not execute the loop.
                match ev_cursor.event(e, f) {
                    (Running, _) => {}
                    x => return x,
                };
                let cur = cursor;
                let mut remaining_dt = upd.unwrap_or(0.0);
                let mut remaining_e;
                loop {
                    match cur.event(
                        match upd {
                            Some(_) => {
                                remaining_e = UpdateEvent::from_dt(remaining_dt, e).unwrap();
                                &remaining_e
                            }
                            _ => e,
                        },
                        f,
                    ) {
                        (Failure, x) => return (Failure, x),
                        (Running, _) => break,
                        (Success, new_dt) => {
                            remaining_dt = match upd {
                                // Change update event with remaining delta time.
                                Some(_) => new_dt,
                                // Other events are 'consumed' and not passed to next.
                                _ => return RUNNING,
                            }
                        }
                    };
                    *i += 1;
                    // If end of repeated events,
                    // start over from the first one.
                    if *i >= rep.len() {
                        *i = 0;
                    }
                    // Create a new cursor for next event.
                    // Use the same pointer to avoid allocation.
                    **cur = State::new(rep[*i].clone());
                }
                RUNNING
            }
            (_, &mut WhenAllState(ref mut cursors)) => {
                let any = false;
                when_all(any, upd, cursors, e, f)
            }
            (_, &mut WhenAnyState(ref mut cursors)) => {
                let any = true;
                when_all(any, upd, cursors, e, f)
            }
            (_, &mut AfterState(ref mut i, ref mut cursors)) => {
                // Get the least delta time left over.
                let mut min_dt = f64::MAX;
                for j in *i..cursors.len() {
                    match cursors[j].event(e, f) {
                        (Running, _) => {
                            min_dt = 0.0;
                        }
                        (Success, new_dt) => {
                            // Remaining delta time must be less to succeed.
                            if *i == j && new_dt < min_dt {
                                *i += 1;
                                min_dt = new_dt;
                            } else {
                                // Return least delta time because
                                // that is when failure is detected.
                                return (Failure, min_dt.min(new_dt));
                            }
                        }
                        (Failure, new_dt) => {
                            return (Failure, new_dt);
                        }
                    };
                }
                if *i == cursors.len() {
                    (Success, min_dt)
                } else {
                    RUNNING
                }
            }
            _ => RUNNING,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Behavior::{Action, Sequence};

    /// Some test actions.
    #[derive(Clone)]
    #[allow(dead_code)]
    pub enum TestActions {
        /// Increment accumulator.
        Inc,
        /// Decrement accumulator.
        Dec,
    }

    use crate::state::tests::TestActions::{Dec, Inc};

    #[test]
    fn test_bt_tick() {
        let seq = Sequence(vec![Action(Inc), Action(Dec), Action(Inc)]);
        let mut state = State::new(seq);

        let mut acc: u32 = 0;
        let f = &mut |args: ActionArgs<Event, TestActions, ()>| match &*args.action {
            Inc => {
                acc += 1;
                (Success, args.dt)
            }
            Dec => {
                acc -= 1;
                (Success, args.dt)
            }
        };

        state.tick(0.0, f);
        assert_eq!(acc, 1);
    }
}