// Software Name: its-client
// SPDX-FileCopyrightText: Copyright (c) 2016-2022 Orange
// SPDX-License-Identifier: MIT License
//
// This software is distributed under the MIT license, see LICENSE.txt file for more details.
//
// Author: Frédéric GARDES <frederic.gardes@orange.com> et al.
// Software description: This Intelligent Transportation Systems (ITS) [MQTT](https://mqtt.org/) client based on the [JSon](https://www.json.org) [ETSI](https://www.etsi.org/committee/its) specification transcription provides a ready to connect project for the mobility (connected and autonomous vehicles, road side units, vulnerable road users,...).

use crate::analyse::cause::Cause;
use crate::analyse::configuration::Configuration;
use crate::analyse::item::Item;
use crate::monitor;
use crate::mqtt::mqtt_client::{listen, Client};
use crate::mqtt::mqtt_router;
use crate::reception::exchange::collective_perception_message::CollectivePerceptionMessage;
use crate::reception::exchange::cooperative_awareness_message::CooperativeAwarenessMessage;
use crate::reception::exchange::decentralized_environmental_notification_message::DecentralizedEnvironmentalNotificationMessage;
use crate::reception::exchange::Exchange;
use crate::reception::information::Information;
use crate::reception::typed::Typed;
use crate::reception::Reception;
use log::{debug, error, info, trace, warn};
use rumqttc::{Event, EventLoop, Publish};
use serde::de::DeserializeOwned;
use std::any::Any;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

pub mod consumer;
pub mod producer;
pub mod async_producer;

pub fn unbox<T>(value: Box<T>) -> T {
    *value
}

fn monitor_thread(
    direction: String,
    configuration: Arc<Configuration>,
    exchange_receiver: Receiver<(Item<Exchange>, Option<Cause>)>,
) -> JoinHandle<()> {
    info!("starting monitor reception thread...");
    let handle = thread::Builder::new()
        .name("monitor-reception".into())
        .spawn(move || {
            trace!("monitor reception entering...");
            for tuple in exchange_receiver {
                let publish_item = tuple.0;
                let cause = tuple.1;
                // monitor
                monitor::monitor(
                    &publish_item.reception,
                    cause,
                    direction.as_str(),
                    // assumed clone, we preserve it for the topics
                    configuration.component_name(None),
                    format!(
                        "{}/{}/{}",
                        configuration.gateway_component_name(),
                        publish_item.topic.project_base(),
                        publish_item.reception.source_uuid
                    ),
                );
            }
        })
        .unwrap();
    info!("monitor reception thread started");
    handle
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

async fn mqtt_client_subscribe(topic_list: &Vec<String>, client: &mut Client) {
    info!("mqtt client subscribing starting...");
    let mut topic_subscription_list = Vec::new();

    for topic in topic_list.iter() {
        match topic {
            info_topic if info_topic.contains(Information::get_type().as_str()) => {
                topic_subscription_list.push(format!("{}/broker", info_topic));
            }
            _ => topic_subscription_list.push(format!("{}/+/#", topic)),
        }
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

fn mqtt_router_dispatch_thread(
    topic_list: Vec<String>,
    event_receiver: Receiver<Event>,
    // FIXME manage a Box into the Exchange to use a unique object Trait instead
) -> (
    Receiver<Item<Exchange>>,
    Receiver<(Item<Exchange>, Option<Cause>)>,
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

            for topic in topic_list.iter() {
                match topic {
                    info_topic if info_topic.contains(Information::get_type().as_str()) => {
                        router.add_route(info_topic, deserialize::<Information>);
                    }
                    _ => router.add_route(topic, deserialize::<Exchange>),
                }
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
                                match monitoring_sender.send((item.clone(), None)) {
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

fn deserialize<T>(publish: rumqttc::Publish) -> Option<Box<dyn Any + 'static + Send>>
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
