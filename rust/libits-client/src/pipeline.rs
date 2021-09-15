use std::any::Any;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use log::{debug, error, info, trace, warn};
use rumqttc::{Event, EventLoop, Publish};
use serde::de::DeserializeOwned;

use crate::analyse::analyser::Analyser;
use crate::analyse::configuration::Configuration;
use crate::analyse::item::Item;
use crate::monitor;
use crate::mqtt::mqtt_client::{listen, Client};
use crate::mqtt::{mqtt_client, mqtt_router};
use crate::reception::exchange::collective_perception_message::CollectivePerceptionMessage;
use crate::reception::exchange::cooperative_awareness_message::CooperativeAwarenessMessage;
use crate::reception::exchange::decentralized_environmental_notification_message::DecentralizedEnvironmentalNotificationMessage;
use crate::reception::exchange::Exchange;
use crate::reception::information::Information;
use crate::reception::typed::Typed;
use crate::reception::Reception;

pub async fn run<T: Analyser>(
    mqtt_host: &str,
    mqtt_port: u16,
    mqtt_client_id: &str,
    mqtt_username: Option<&str>,
    mqtt_password: Option<&str>,
    mqtt_root_topic: &str,
) {
    loop {
        // build the shared topic list
        let topic_list = vec![
            format!("{}/v2x/cam", mqtt_root_topic),
            format!("{}/v2x/cpm", mqtt_root_topic),
            format!("{}/v2x/denm", mqtt_root_topic),
            format!("{}/info", mqtt_root_topic),
        ];

        //initialize the client
        let (mut client, event_loop) = mqtt_client::Client::new(
            mqtt_host,
            mqtt_port,
            mqtt_client_id,
            mqtt_username,
            mqtt_password,
        );

        let configuration = Arc::new(Configuration::new(mqtt_client_id.to_string()));

        // subscribe
        mqtt_client_subscribe(&topic_list, &mut client).await;
        // receive
        let (event_receiver, mqtt_client_listen_handle) = mqtt_client_listen_thread(event_loop);
        // dispatch
        let (item_receiver, monitoring_receiver, information_receiver, mqtt_router_dispatch_handle) =
            mqtt_router_dispatch_thread(topic_list, event_receiver);

        // in parallel, monitor exchanges reception
        let monitor_reception_handle = monitor_thread(
            "received_on".to_string(),
            configuration.clone(),
            monitoring_receiver,
        );
        // in parallel, analyse exchanges
        let (publish_item_receiver, publish_monitoring_receiver, analyser_generate_handle) =
            analyser_generate_thread::<T>(configuration.clone(), item_receiver);

        // read information
        let reader_configure_handle =
            reader_configure_thread(configuration.clone(), information_receiver);

        // in parallel, monitor exchanges publish
        let monitor_publish_handle = monitor_thread(
            "sent_on".to_string(),
            configuration.clone(),
            publish_monitoring_receiver,
        );
        // in parallel send
        mqtt_client_publish(publish_item_receiver, &mut client).await;

        debug!("mqtt_client_listen_handler joining...");
        mqtt_client_listen_handle.await.unwrap();
        debug!("mqtt_router_dispatch_handler joining...");
        mqtt_router_dispatch_handle.join().unwrap();
        debug!("monitor_reception_handle joining...");
        monitor_reception_handle.join().unwrap();
        debug!("reader_configure_handler joining...");
        reader_configure_handle.join().unwrap();
        debug!("analyser_generate_handler joining...");
        analyser_generate_handle.join().unwrap();
        debug!("monitor_publish_handle joining...");
        monitor_publish_handle.join().unwrap();

        warn!("loop done");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn mqtt_client_listen_thread(
    event_loop: EventLoop,
) -> (Receiver<Event>, tokio::task::JoinHandle<()>) {
    info!("starting mqtt client listening...");
    let (event_sender, event_receiver) = channel();
    let handle = tokio::task::spawn(async move {
        trace!("mqtt client listening closure entering...");
        listen(event_loop, event_sender).await;
        trace!("mqtt client listening closure finished");
    });
    info!("mqtt client listening started");
    (event_receiver, handle)
}

fn mqtt_router_dispatch_thread(
    topic_list: Vec<String>,
    event_receiver: Receiver<Event>,
    // FIXME manage a Box into the Exchange to use a unique object Trait instead
) -> (
    Receiver<Item<Exchange>>,
    Receiver<Item<Exchange>>,
    Receiver<Item<Information>>,
    JoinHandle<()>,
) {
    info!("starting mqtt router dispatching...");
    let (exchange_sender, exchange_receiver) = channel();
    let (monitoring_sender, monitoring_receiver) = channel();
    let (information_sender, information_receiver) = channel();

    let handle = thread::Builder::new()
        .name("mqtt-router-dispatcher".into())
        .spawn(move || {
            trace!("mqtt router dispatching closure entering...");
            //initialize the router
            let router = &mut mqtt_router::Router::new();

            if let Some(cam_topic) = topic_list
                .iter()
                .find(|&r| r.contains(CooperativeAwarenessMessage::get_type().as_str()))
            {
                router.add_route(cam_topic, deserialize::<Exchange>);
            }
            if let Some(denm_topic) = topic_list.iter().find(|&r| {
                r.contains(DecentralizedEnvironmentalNotificationMessage::get_type().as_str())
            }) {
                router.add_route(denm_topic, deserialize::<Exchange>);
            }
            if let Some(cpm_topic) = topic_list
                .iter()
                .find(|&r| r.contains(CollectivePerceptionMessage::get_type().as_str()))
            {
                router.add_route(cpm_topic, deserialize::<Exchange>);
            }
            if let Some(info_topic) = topic_list
                .iter()
                .find(|&r| r.contains(Information::get_type().as_str()))
            {
                router.add_route(info_topic, deserialize::<Information>);
            }

            for event in event_receiver {
                match router.handle_event(event) {
                    Some((topic, reception)) => {
                        // TODO use the From Trait
                        if reception.is::<Exchange>() {
                            if let Ok(exchange) = reception.downcast::<Exchange>() {
                                let item = Item {
                                    topic,
                                    reception: unbox(exchange),
                                };
                                //assumed clone, we send to 2 channels
                                match monitoring_sender.send(item.clone()) {
                                    Ok(()) => trace!("mqtt monitoring sent"),
                                    Err(error) => {
                                        error!("stopped to send mqtt monitoring: {}", error);
                                        break;
                                    }
                                }
                                match exchange_sender.send(item) {
                                    Ok(()) => trace!("mqtt exchange sent"),
                                    Err(error) => {
                                        error!("stopped to send mqtt exchange: {}", error);
                                        break;
                                    }
                                }
                            }
                        } else if let Ok(information) = reception.downcast::<Information>() {
                            match information_sender.send(Item {
                                topic,
                                reception: unbox(information),
                            }) {
                                Ok(()) => trace!("mqtt information sent"),
                                Err(error) => {
                                    error!("stopped to send mqtt information: {}", error);
                                    break;
                                }
                            }
                        }
                    }
                    None => trace!("no mqtt response to send"),
                }
            }
            trace!("mqtt router dispatching closure finished");
        })
        .unwrap();
    info!("mqtt router dispatching started");
    (
        exchange_receiver,
        monitoring_receiver,
        information_receiver,
        handle,
    )
}

fn monitor_thread(
    direction: String,
    configuration: Arc<Configuration>,
    exchange_receiver: Receiver<Item<Exchange>>,
) -> JoinHandle<()> {
    info!("starting monitor reception thread...");
    let handle = thread::Builder::new()
        .name("monitor-reception".into())
        .spawn(move || {
            trace!("monitor reception entering...");
            for item in exchange_receiver {
                // monitor
                monitor::monitor(
                    &item.reception,
                    direction.as_str(),
                    // assumed clone, we preserve it for the topics
                    configuration.component_name(None),
                    format!(
                        "{}/{}/{}",
                        configuration.gateway_component_name(),
                        item.topic.project_base(),
                        item.reception.source_uuid
                    ),
                );
            }
        })
        .unwrap();
    info!("monitor reception thread started");
    handle
}

pub fn unbox<T>(value: Box<T>) -> T {
    *value
}

fn analyser_generate_thread<T: Analyser>(
    configuration: Arc<Configuration>,
    exchange_receiver: Receiver<Item<Exchange>>,
) -> (
    Receiver<Item<Exchange>>,
    Receiver<Item<Exchange>>,
    JoinHandle<()>,
) {
    info!("starting analyser generation...");
    let (publish_sender, publish_receiver) = channel();
    let (monitoring_sender, monitoring_receiver) = channel();
    let handle = thread::Builder::new()
        .name("analyser-generator".into())
        .spawn(move || {
            trace!("analyser generation closure entering...");
            //initialize the analyser
            let mut analyser = T::new(configuration);
            for item in exchange_receiver {
                for publish_item in analyser.analyze(item) {
                    //assumed clone, we send to 2 channels
                    match publish_sender.send(publish_item.clone()) {
                        Ok(()) => trace!("publish sent"),
                        Err(error) => {
                            error!("stopped to send publish: {}", error);
                            break;
                        }
                    }
                    match monitoring_sender.send(publish_item) {
                        Ok(()) => trace!("monitoring sent"),
                        Err(error) => {
                            error!("stopped to send monitoring: {}", error);
                            break;
                        }
                    }
                }
                trace!("analyser generation closure finished");
            }
        })
        .unwrap();
    info!("analyser generation started");
    (publish_receiver, monitoring_receiver, handle)
}

fn reader_configure_thread(
    configuration: Arc<Configuration>,
    information_receiver: Receiver<Item<Information>>,
) -> JoinHandle<()> {
    info!("starting reader configuration...");
    let handle = thread::Builder::new()
        .name("reader-configurator".into())
        .spawn(move || {
            trace!("reader configuration closure entering...");
            for item in information_receiver {
                info!(
                    "we received an information on the topic {}: {:?}",
                    item.topic, item.reception
                );
                configuration.update(item.reception);
            }
            trace!("reader configuration closure finished");
        })
        .unwrap();
    info!("reader configuration started");
    handle
}

fn deserialize<T>(publish: Publish) -> Option<Box<dyn Any + 'static + Send>>
where
    T: DeserializeOwned + Reception + 'static + Send,
{
    // Incoming publish from the broker
    match String::from_utf8(publish.payload.to_vec()) {
        Ok(message) => {
            let message_str = message.as_str();
            match serde_json::from_str::<T>(message_str) {
                Ok(message) => {
                    trace!("message parsed");
                    return Some(Box::new(message));
                }
                Err(e) => warn!("parse error({}) on: {}", e, message_str),
            }
        }
        Err(e) => warn!("format error: {}", e),
    }
    Option::None
}

async fn mqtt_client_subscribe(topic_list: &Vec<String>, client: &mut Client) {
    info!("mqtt client subscribing starting...");
    // build the topic subscription list
    let mut topic_subscription_list = Vec::new();
    if let Some(cam_topic) = topic_list
        .iter()
        .find(|&r| r.contains(CooperativeAwarenessMessage::get_type().as_str()))
    {
        topic_subscription_list.push(format!("{}/+/#", cam_topic));
    }
    if let Some(denm_topic) = topic_list
        .iter()
        .find(|&r| r.contains(DecentralizedEnvironmentalNotificationMessage::get_type().as_str()))
    {
        topic_subscription_list.push(format!("{}/+/#", denm_topic));
    }
    if let Some(cpm_topic) = topic_list
        .iter()
        .find(|&r| r.contains(CollectivePerceptionMessage::get_type().as_str()))
    {
        topic_subscription_list.push(format!("{}/+/#", cpm_topic));
    }
    if let Some(info_topic) = topic_list
        .iter()
        .find(|&r| r.contains(Information::get_type().as_str()))
    {
        // The topic of the broker we are currently connected to
        // is always: "5GCroCo/backOutQueue/info/broker"
        topic_subscription_list.push(format!("{}/broker", info_topic));
    }

    // NOTE: we share the topic list with the dispatcher
    client.subscribe(topic_subscription_list).await;
    info!("mqtt client subscribing finished");
}

async fn mqtt_client_publish(publish_item_receiver: Receiver<Item<Exchange>>, client: &mut Client) {
    info!("mqtt client publishing starting...");
    for item in publish_item_receiver {
        debug!("we received a publish");
        client.publish(item).await;
        debug!("we forwarded the publish");
    }
    info!("mqtt client publishing finished");
}
