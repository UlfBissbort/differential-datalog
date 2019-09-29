use differential_datalog::record::{Record, RelIdentifier, UpdCmd};
use observe::Observable;
use observe::Observer;
use observe::SharedObserver;

use server_api_ddlog::api::*;
use server_api_ddlog::server;
use server_api_ddlog::Relations::*;

use maplit::hashmap;
use maplit::hashset;

#[derive(Clone, Debug)]
struct Mock {
    called_on_start: usize,
    called_on_commit: usize,
    called_on_updates: usize,
    called_on_completed: usize,
}

impl Mock {
    fn new() -> Self {
        Self {
            called_on_start: 0,
            called_on_commit: 0,
            called_on_updates: 0,
            called_on_completed: 0,
        }
    }
}

impl<T, E> Observer<T, E> for Mock
where
    T: Send,
    E: Send,
{
    fn on_start(&mut self) -> Result<(), E> {
        self.called_on_start += 1;
        Ok(())
    }

    fn on_commit(&mut self) -> Result<(), E> {
        self.called_on_commit += 1;
        Ok(())
    }

    fn on_updates<'a>(&mut self, updates: Box<dyn Iterator<Item = T> + 'a>) -> Result<(), E> {
        self.called_on_updates += updates.count();
        Ok(())
    }

    fn on_completed(&mut self) -> Result<(), E> {
        self.called_on_completed += 1;
        Ok(())
    }
}

/// Verify that `on_commit` is called even if we haven't received any
/// updates.
#[test]
fn start_commit_on_no_updates() -> Result<(), String> {
    let program = HDDlog::run(1, false, |_, _: &Record, _| {});
    let mut server = server::DDlogServer::new(program, hashmap! {});

    let observable = SharedObserver::new(Mock::new());
    let mut stream = server.add_stream(hashset! {});
    let _ = stream
        .subscribe(Box::new(observable.clone()))
        .ok_or_else(|| "failed to subscribe observer because a subscriber is already present")?;

    server.on_start()?;
    server.on_commit()?;
    server.on_completed()?;

    {
        let mock = observable.0.lock().unwrap();
        assert_eq!(mock.called_on_start, 1);
        assert_eq!(mock.called_on_commit, 1);
    }

    server.shutdown()?;

    // Also verify that on_completed is called as part of the shutdown
    // procedure.
    {
        let mock = observable.0.lock().unwrap();
        assert_eq!(mock.called_on_completed, 1);
    }

    server.shutdown()?;

    // But only once!
    {
        let mock = observable.0.lock().unwrap();
        assert_eq!(mock.called_on_completed, 1);
    }
    Ok(())
}

/// Verify that we receive an `on_updates` (and `on_next`) call when we
/// get an update.
#[test]
fn start_commit_with_updates() -> Result<(), String> {
    let program = HDDlog::run(1, false, |_, _: &Record, _| {});
    let mut server = server::DDlogServer::new(program, hashmap! {});

    let observable = SharedObserver::new(Mock::new());
    let mut stream = server.add_stream(hashset! {P1Out});
    let _ = stream
        .subscribe(Box::new(observable.clone()))
        .ok_or_else(|| "failed to subscribe observer because a subscriber is already present")?;

    let updates = &[UpdCmd::Insert(
        RelIdentifier::RelId(P1In as usize),
        Record::String("test".to_string()),
    )];

    server.on_start()?;
    server.on_updates(Box::new(
        updates.into_iter().map(|cmd| updcmd2upd(cmd).unwrap()),
    ))?;
    server.on_commit()?;
    server.on_completed()?;

    let mock = observable.0.lock().unwrap();
    assert_eq!(mock.called_on_start, 1);
    assert_eq!(mock.called_on_updates, 1);
    assert_eq!(mock.called_on_commit, 1);
    Ok(())
}

/// Test `unsubscribe` functionality.
#[test]
fn unsubscribe() -> Result<(), String> {
    let program = HDDlog::run(1, false, |_, _: &Record, _| {});
    let mut server = server::DDlogServer::new(program, hashmap! {});

    let observable = SharedObserver::new(Mock::new());
    let mut stream = server.add_stream(hashset! {P1Out});
    let subscription = stream
        .subscribe(Box::new(observable.clone()))
        .ok_or_else(|| "failed to subscribe observer because a subscriber is already present")?;

    server.on_start()?;
    server.on_commit()?;

    subscription.unsubscribe();

    server.on_start()?;
    server.on_commit()?;

    let mock = observable.0.lock().unwrap();
    assert_eq!(mock.called_on_start, 1);
    assert_eq!(mock.called_on_commit, 1);
    Ok(())
}

/// Verify that we do not receive repeated `on_next` calls for inserts &
/// deletes of the same object within a single transaction.
#[test]
fn multiple_mergable_updates() -> Result<(), String> {
    let program = HDDlog::run(1, false, |_, _: &Record, _| {});
    let mut server = server::DDlogServer::new(program, hashmap! {});

    let observable = SharedObserver::new(Mock::new());
    let mut stream = server.add_stream(hashset! {P1Out});
    let _ = stream
        .subscribe(Box::new(observable.clone()))
        .ok_or_else(|| "failed to subscribe observer because a subscriber is already present")?;

    let updates = &[
        UpdCmd::Insert(
            RelIdentifier::RelId(P1In as usize),
            Record::String("42".to_string()),
        ),
        UpdCmd::Insert(
            RelIdentifier::RelId(P1In as usize),
            Record::String("here-to-stay".to_string()),
        ),
        UpdCmd::Delete(
            RelIdentifier::RelId(P1In as usize),
            Record::String("42".to_string()),
        ),
    ];

    server.on_start()?;
    server.on_updates(Box::new(
        updates.into_iter().map(|cmd| updcmd2upd(cmd).unwrap()),
    ))?;
    server.on_commit()?;
    server.on_completed()?;

    let mock = observable.0.lock().unwrap();
    assert_eq!(mock.called_on_start, 1);
    assert_eq!(mock.called_on_updates, 1);
    assert_eq!(mock.called_on_commit, 1);
    Ok(())
}

/// Check intended behavior in the context of multiple transactions
/// happening.
#[test]
fn multiple_transactions() -> Result<(), String> {
    let program = HDDlog::run(1, false, |_, _: &Record, _| {});
    let mut server = server::DDlogServer::new(program, hashmap! {});

    let observable = SharedObserver::new(Mock::new());
    let mut stream = server.add_stream(hashset! {P1Out});
    let _ = stream.subscribe(Box::new(observable.clone()));

    let updates = &[
        UpdCmd::Insert(
            RelIdentifier::RelId(P1In as usize),
            Record::String("first".to_string()),
        ),
        UpdCmd::Insert(
            RelIdentifier::RelId(P1In as usize),
            Record::String("second".to_string()),
        ),
    ];

    server.on_start()?;
    server.on_updates(Box::new(
        updates.into_iter().map(|cmd| updcmd2upd(cmd).unwrap()),
    ))?;
    server.on_commit()?;

    {
        let mock = observable.0.lock().unwrap();
        assert_eq!(mock.called_on_start, 1);
        assert_eq!(mock.called_on_updates, 2);
        assert_eq!(mock.called_on_commit, 1);
    }

    let updates = &[UpdCmd::Delete(
        RelIdentifier::RelId(P1In as usize),
        Record::String("first".to_string()),
    )];

    server.on_start()?;
    server.on_updates(Box::new(
        updates.into_iter().map(|cmd| updcmd2upd(cmd).unwrap()),
    ))?;
    server.on_commit()?;

    {
        let mock = observable.0.lock().unwrap();
        assert_eq!(mock.called_on_start, 2);
        assert_eq!(mock.called_on_updates, 3);
        assert_eq!(mock.called_on_commit, 2);
    }

    server.on_completed()?;
    Ok(())
}