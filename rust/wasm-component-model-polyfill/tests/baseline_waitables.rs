//! Baseline tests for the store's waitable and waitable set records.
//!
//! A waitable is a handle a guest can wait on; a subtask is the one
//! kind the polyfill builds today. The records carry one pending
//! event slot each, the set the waitable joined, and the flag that
//! marks a synchronous waiter, and a set carries its waitables in
//! join order with the count of threads waiting on it. No built-in
//! reaches them yet, so the tests drive the store operations the
//! built-ins of the later features call.

#![cfg(test)]

use wasm_component_model_polyfill::{Engine, Error, ResourceTypeId, Store, WaitableCause};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A store with nothing instantiated in it: the records under test
/// are the store's own, and no component is needed to reach them.
fn store() -> Store<()> {
    let engine = Engine::new().expect("engine");
    Store::new(&engine, ()).expect("store")
}

#[wcmp_macros::test]
async fn it_delivers_the_events_of_one_set_in_join_order() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let first = tables.tasks.insert_subtask();
    let second = tables.tasks.insert_subtask();
    let first_waitable = tables.tasks.subtask_waitable(first);
    let second_waitable = tables.tasks.subtask_waitable(second);
    tables
        .tasks
        .join_waitable_set(first_waitable, Some(set))
        .expect("the first subtask joins the set");
    tables
        .tasks
        .join_waitable_set(second_waitable, Some(set))
        .expect("the second subtask joins the set");

    // Both calls return, and the later joiner is made ready first:
    // the set delivers in join order, not in the order the events
    // were filled.
    tables.tasks.subtask_returned(first).expect("first returns");
    tables
        .tasks
        .subtask_returned(second)
        .expect("second returns");
    tables
        .tasks
        .record_subtask_event(second, 9)
        .expect("the second subtask is ready");
    tables
        .tasks
        .record_subtask_event(first, 7)
        .expect("the first subtask is ready");

    assert_eq!(
        tables.poll_waitable_set(set).expect("poll").triple(),
        (1, 7, 2),
        "the subtask that joined first delivers first, with its handle index and returned state"
    );
    assert_eq!(
        tables.poll_waitable_set(set).expect("poll").triple(),
        (1, 9, 2),
        "the subtask that joined second delivers second"
    );
    assert_eq!(
        tables.poll_waitable_set(set).expect("poll").triple(),
        (0, 0, 0),
        "a set that holds no event answers with the none event"
    );
}

#[wcmp_macros::test]
async fn it_returns_at_once_from_a_wait_on_a_set_that_already_holds_an_event() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let instance = tables.tasks.insert_instance();
    let task = tables.tasks.push_task(None, None, instance);
    let thread = tables.tasks.current_thread().expect("the task's thread");
    let set = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");
    tables
        .tasks
        .subtask_returned(subtask)
        .expect("the call returns");
    tables
        .tasks
        .record_subtask_event(subtask, 3)
        .expect("the subtask is ready");

    let delivered = tables
        .wait_on_waitable_set(set, thread)
        .expect("wait")
        .expect("the set already holds an event, so the wait returns at once");
    assert_eq!(delivered.triple(), (1, 3, 2));
    assert_eq!(
        tables.tasks.waitable_set(set).expect("the set").num_waiting,
        0,
        "a wait that returned at once left no waiter behind"
    );
    assert!(
        !tables
            .tasks
            .has_pending_event(waitable)
            .expect("the waitable"),
        "delivery emptied the pending event slot"
    );

    // With the slot empty the same wait blocks: the thread is parked
    // on the set until its wait ends.
    assert!(
        tables
            .wait_on_waitable_set(set, thread)
            .expect("wait")
            .is_none(),
        "a set that holds no event cannot deliver, so the thread waits"
    );
    assert_eq!(
        tables.tasks.waitable_set(set).expect("the set").num_waiting,
        1,
        "the set counts the waiting thread"
    );
    assert_eq!(
        tables
            .finish_wait_on_waitable_set(set, thread)
            .expect("the wait ends")
            .triple(),
        (0, 0, 0),
        "nothing arrived while the thread waited"
    );
    assert_eq!(
        tables.tasks.waitable_set(set).expect("the set").num_waiting,
        0,
        "the waiter is gone once its wait ended"
    );

    let _ = tables.exit_task(task);
}

#[wcmp_macros::test]
async fn it_moves_a_waitable_out_of_its_previous_set_when_it_joins_another() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let first = tables.tasks.insert_waitable_set();
    let second = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);

    tables
        .tasks
        .join_waitable_set(waitable, Some(first))
        .expect("the subtask joins the first set");
    assert_eq!(
        tables.tasks.waitable_set(first).expect("the set").waitables,
        vec![waitable]
    );
    assert_eq!(
        tables
            .tasks
            .waitable_set_of(waitable)
            .expect("the waitable"),
        Some(first)
    );

    tables
        .tasks
        .join_waitable_set(waitable, Some(second))
        .expect("the subtask joins the second set");
    assert!(
        tables
            .tasks
            .waitable_set(first)
            .expect("the set")
            .waitables
            .is_empty(),
        "joining the second set removed the waitable from the first"
    );
    assert_eq!(
        tables
            .tasks
            .waitable_set(second)
            .expect("the set")
            .waitables,
        vec![waitable]
    );
    assert_eq!(
        tables
            .tasks
            .waitable_set_of(waitable)
            .expect("the waitable"),
        Some(second)
    );
}

#[wcmp_macros::test]
async fn it_traps_when_a_waitable_with_a_synchronous_waiter_joins_a_set() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .begin_synchronous_wait(waitable)
        .expect("a thread waits on the subtask on its own");

    let error = tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect_err("a waitable with a synchronous waiter cannot join a set");
    assert!(matches!(
        error,
        Error::Waitable(WaitableCause::SyncAndAsync)
    ));
    assert_eq!(
        error.to_string(),
        "waitable error: waitable cannot be used synchronously while added to a waitable set"
    );

    tables
        .tasks
        .end_synchronous_wait(waitable)
        .expect("the synchronous wait ends");
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set once its waiter is gone");
    assert!(
        matches!(
            tables
                .tasks
                .begin_synchronous_wait(waitable)
                .expect_err("and the rule holds in the other direction"),
            Error::Waitable(WaitableCause::SyncAndAsync)
        ),
        "a waitable in a set cannot take a synchronous waiter either"
    );
}

#[wcmp_macros::test]
async fn it_traps_when_a_set_that_still_holds_waitables_is_dropped() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");

    let error = tables
        .tasks
        .drop_waitable_set(set)
        .expect_err("the set still holds a waitable");
    assert!(matches!(
        error,
        Error::Waitable(WaitableCause::SetHasWaitables)
    ));
    assert_eq!(
        error.to_string(),
        "waitable error: cannot drop waitable set with waitables in it"
    );

    tables
        .tasks
        .join_waitable_set(waitable, None)
        .expect("the subtask leaves the set");
    tables
        .tasks
        .drop_waitable_set(set)
        .expect("an empty set no thread waits on drops");
    assert_eq!(
        tables.tasks.waitable_set_count(),
        0,
        "the set's record is gone"
    );
}

#[wcmp_macros::test]
async fn it_traps_when_a_set_a_thread_is_waiting_on_is_dropped() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let instance = tables.tasks.insert_instance();
    let task = tables.tasks.push_task(None, None, instance);
    let thread = tables.tasks.current_thread().expect("the task's thread");
    let set = tables.tasks.insert_waitable_set();
    assert!(
        tables
            .wait_on_waitable_set(set, thread)
            .expect("wait")
            .is_none(),
        "an empty set delivers nothing, so the thread waits"
    );

    let error = tables
        .tasks
        .drop_waitable_set(set)
        .expect_err("a thread is waiting on the set");
    assert!(matches!(
        error,
        Error::Waitable(WaitableCause::SetHasWaiters)
    ));
    assert_eq!(
        error.to_string(),
        "waitable error: cannot drop waitable set with waiters"
    );

    tables
        .finish_wait_on_waitable_set(set, thread)
        .expect("the wait ends");
    tables
        .tasks
        .drop_waitable_set(set)
        .expect("the set drops once nothing waits on it");

    let _ = tables.exit_task(task);
}

#[wcmp_macros::test]
async fn it_traps_when_a_subtask_whose_resolution_was_not_delivered_is_dropped() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");
    tables
        .tasks
        .subtask_returned(subtask)
        .expect("the call returns");
    tables
        .tasks
        .record_subtask_event(subtask, 4)
        .expect("the subtask is ready");

    let error = tables
        .tasks
        .drop_waitable(waitable)
        .expect_err("the resolution was not delivered");
    assert!(matches!(
        error,
        Error::Waitable(WaitableCause::SubtaskNotResolved)
    ));
    assert_eq!(
        error.to_string(),
        "waitable error: cannot drop a subtask which has not yet resolved"
    );

    assert_eq!(
        tables.poll_waitable_set(set).expect("poll").triple(),
        (1, 4, 2),
        "the poll delivers the subtask event, and with it the resolution"
    );
    tables
        .tasks
        .drop_waitable(waitable)
        .expect("a subtask whose resolution was delivered drops");
    assert_eq!(
        tables.tasks.subtask_count(),
        0,
        "the subtask's record is gone"
    );
    assert!(
        tables
            .tasks
            .waitable_set(set)
            .expect("the set")
            .waitables
            .is_empty(),
        "the dropped subtask left the set on its way out"
    );
}

#[wcmp_macros::test]
async fn it_decrements_the_lenders_of_a_subtask_when_its_resolution_is_delivered() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    // A table of the store to hold the caller's handles. Nothing is
    // instantiated here, so the store's own table for a resource type
    // stands in for the caller instance's table.
    let ty = ResourceTypeId::fresh();
    let table = tables.host_table(ty);
    let owned = tables.insert_own(table, ty, false, 11);

    // The call lends the owned handle while it lifts its parameters,
    // and then leaves the stack: an asynchronous call outlives the
    // scope that started it.
    let subtask = tables.tasks.push_subtask();
    assert_eq!(tables.lend(table, owned), Ok(()), "the borrow lifts out");
    let _ = tables.tasks.pop_scope();
    assert!(
        tables.remove_own(table, owned, ty, false).is_err(),
        "the handle is lent while the call is in flight"
    );

    let set = tables.tasks.insert_waitable_set();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");
    tables
        .tasks
        .subtask_returned(subtask)
        .expect("the call returns");
    let handle_index = tables.insert_subtask(table, subtask);
    tables
        .tasks
        .record_subtask_event(subtask, handle_index)
        .expect("the subtask is ready");
    assert!(
        tables.remove_own(table, owned, ty, false).is_err(),
        "the resolution has not been delivered yet"
    );

    let delivered = tables.poll_waitable_set(set).expect("poll").triple();
    assert_eq!(delivered, (1, handle_index, 2));
    assert_eq!(
        tables.remove_own(table, owned, ty, false),
        Ok(11),
        "delivering the subtask event delivered the resolution, which gave the handle back"
    );
}

#[wcmp_macros::test]
async fn it_finds_a_waitable_and_a_waitable_set_through_their_handles() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let ty = ResourceTypeId::fresh();
    let table = tables.host_table(ty);
    let subtask = tables.tasks.insert_subtask();
    let set = tables.tasks.insert_waitable_set();
    let subtask_handle = tables.insert_subtask(table, subtask);
    let set_handle = tables.insert_waitable_set(table, set);

    assert_eq!(
        tables
            .waitable_from_handle(table, subtask_handle)
            .expect("the subtask handle names a waitable"),
        tables.tasks.subtask_waitable(subtask)
    );
    assert_eq!(
        tables
            .waitable_set_from_handle(table, set_handle)
            .expect("the set handle names a set"),
        set
    );
    assert!(
        tables.waitable_from_handle(table, set_handle).is_err(),
        "a waitable set is not a waitable"
    );
    assert!(
        tables
            .waitable_set_from_handle(table, subtask_handle)
            .is_err(),
        "a subtask is not a waitable set"
    );
}

#[wcmp_macros::test]
async fn it_leaves_a_waitable_in_its_set_when_a_join_names_a_set_that_is_gone() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let joined = tables.tasks.insert_waitable_set();
    let gone = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(joined))
        .expect("the subtask joins the first set");
    tables
        .tasks
        .drop_waitable_set(gone)
        .expect("the second set drops while it holds nothing");

    let error = tables
        .tasks
        .join_waitable_set(waitable, Some(gone))
        .expect_err("the store holds no such set any more");
    assert_eq!(
        error.to_string(),
        "polyfill internal invariant violated: waitable set record is not in the store"
    );
    assert_eq!(
        tables
            .tasks
            .waitable_set(joined)
            .expect("the set")
            .waitables,
        vec![waitable],
        "the join that failed left the waitable listed in the set it already named"
    );
    assert_eq!(
        tables
            .tasks
            .waitable_set_of(waitable)
            .expect("the waitable"),
        Some(joined),
        "and left the waitable naming that set"
    );
}

#[wcmp_macros::test]
async fn it_leaves_the_waiter_count_alone_when_a_wait_names_a_thread_that_is_gone() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let instance = tables.tasks.insert_instance();
    let task = tables.tasks.push_task(None, None, instance);
    let thread = tables.tasks.current_thread().expect("the task's thread");
    let set = tables.tasks.insert_waitable_set();
    let _ = tables.exit_task(task);

    let error = tables
        .tasks
        .begin_wait(set, thread)
        .expect_err("the thread left the store with the task that ran it");
    assert_eq!(
        error.to_string(),
        "polyfill internal invariant violated: waiting thread is not in the store"
    );
    assert_eq!(
        tables.tasks.waitable_set(set).expect("the set").num_waiting,
        0,
        "the wait that failed raised no waiter count"
    );
    tables
        .tasks
        .drop_waitable_set(set)
        .expect("so nothing waits on the set and it drops");
}

#[wcmp_macros::test]
async fn it_refuses_to_take_a_subtask_event_before_its_resolution_is_delivered() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.insert_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");
    tables
        .tasks
        .subtask_returned(subtask)
        .expect("the call returns");
    tables
        .tasks
        .record_subtask_event(subtask, 6)
        .expect("the subtask is ready");

    let error = tables
        .tasks
        .take_pending_event(waitable)
        .expect_err("the record operation on its own cannot deliver the resolution");
    assert_eq!(
        error.to_string(),
        "polyfill internal invariant violated: a subtask's event was taken before its resolution was delivered"
    );
    assert!(
        tables
            .tasks
            .has_pending_event(waitable)
            .expect("the waitable"),
        "the refused take left the event where it was"
    );
    assert!(
        tables.tasks.drop_waitable(waitable).is_err(),
        "and left the subtask's resolution undelivered"
    );

    assert_eq!(
        tables.poll_waitable_set(set).expect("poll").triple(),
        (1, 6, 2),
        "the paired operation delivers the resolution and takes the event together"
    );
    assert!(
        tables
            .tasks
            .take_pending_event(waitable)
            .expect("a subtask whose resolution was delivered owes nothing")
            .is_none(),
        "and the delivery emptied the slot"
    );
}

#[wcmp_macros::test]
async fn it_leaves_the_set_of_a_subtask_its_own_exit_removed() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let subtask = tables.tasks.push_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");

    tables.abandon_subtask(subtask);
    assert_eq!(
        tables.tasks.subtask_count(),
        0,
        "the exit took the subtask's record with it"
    );

    let reused = tables.tasks.insert_subtask();
    assert_eq!(
        reused.index(),
        subtask.index(),
        "the freed index is handed out again"
    );
    assert!(
        tables
            .tasks
            .waitable_set(set)
            .expect("the set")
            .waitables
            .is_empty(),
        "the removed subtask left the set, so the record that took its index is not in it"
    );
    assert_eq!(
        tables
            .tasks
            .waitable_set_of(tables.tasks.subtask_waitable(reused))
            .expect("the waitable"),
        None,
        "and the record that took the index names no set"
    );
    tables
        .tasks
        .drop_waitable_set(set)
        .expect("a set nothing is in drops");
}

#[wcmp_macros::test]
async fn it_leaves_the_set_of_a_subtask_a_discarded_scope_removed() {
    let store = store();
    let mut guard = store.tables().lock().expect("handle tables");
    let tables = &mut *guard;

    let set = tables.tasks.insert_waitable_set();
    let instance = tables.tasks.insert_instance();
    let task = tables.tasks.push_task(None, None, instance);
    let subtask = tables.tasks.push_subtask();
    let waitable = tables.tasks.subtask_waitable(subtask);
    tables
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");

    // The call failed between the push of the subtask and the pop
    // that would have ended it, so the task's exit discards the
    // scope the failure left above it.
    let _ = tables.exit_task(task);
    assert_eq!(
        tables.tasks.subtask_count(),
        0,
        "the discarded scope took the subtask's record with it"
    );

    let reused = tables.tasks.insert_subtask();
    assert_eq!(
        reused.index(),
        subtask.index(),
        "the freed index is handed out again"
    );
    assert!(
        tables
            .tasks
            .waitable_set(set)
            .expect("the set")
            .waitables
            .is_empty(),
        "the discarded subtask left the set, so the record that took its index is not in it"
    );
    tables
        .tasks
        .drop_waitable_set(set)
        .expect("a set nothing is in drops");
}
