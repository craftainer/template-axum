//! `lib::build_event_bus`'s real-broker branch against the devcontainer
//! stack's Mosquitto: the app's own wiring (host/port from `Settings`, the
//! `mqtt` feature of `craftainer-crud`) delivers a published event to a
//! subscriber. The bus's behaviour (replay, subscriber ids, the anonymous
//! cap) is `craftainer-crud`'s own test suite (docs/adrs/0020); this only
//! proves the app hands it the right broker.

use std::time::Duration;

use craftainer_crud::events::{CrudEvent, EventAction};
use template_axum::build_event_bus;

mod common;

#[tokio::test]
async fn the_dev_mode_bus_delivers_a_published_event_through_the_real_broker() {
    let bus = build_event_bus(&common::dev_settings(None), None);
    let resource = format!("heroes-it-{}", common::unique_suffix());

    // Transient: a clean session, so nothing is left on the shared broker.
    let (_id, mut stream) = bus
        .subscribe_transient(&resource, None)
        .await
        .expect("the stack's MQTT broker must be running for the integration tier");
    bus.publish(CrudEvent::new(&resource, EventAction::Create, vec![7]))
        .await;

    let event = tokio::time::timeout(Duration::from_secs(10), stream.next_event())
        .await
        .expect("the event should arrive within 10s")
        .expect("the stream should yield the event");
    assert_eq!(event.resource, resource);
    assert_eq!(event.ids, vec![7]);
}
