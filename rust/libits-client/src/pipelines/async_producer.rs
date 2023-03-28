// Software Name: its-client
// SPDX-FileCopyrightText: Copyright (c) 2016-2022 Orange
// SPDX-License-Identifier: MIT License
//
// This software is distributed under the MIT license, see LICENSE.txt file for more details.
//
// Author: Nicolas BUFFON <nicolas.buffon@orange.com> et al.
// Software description: This Intelligent Transportation Systems (ITS) [MQTT](https://mqtt.org/) client based on the [JSon](https://www.json.org) [ETSI](https://www.etsi.org/committee/its) specification transcription provides a ready to connect project for the mobility (connected and autonomous vehicles, road side units, vulnerable road users,...).

use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use threadpool::ThreadPool;

use log::{debug, error, info, trace, warn};
use rumqttc::{Event, EventLoop, Publish};
use serde::de::DeserializeOwned;

use crate::analyse::analyser::{Analyser, StatefulAnalyzer};
use crate::analyse::cause::Cause;
use crate::analyse::configuration::Configuration;
use crate::analyse::item::Item;
use crate::monitor;
use crate::mqtt::mqtt_client::{listen, Client};
use crate::mqtt::mqtt_router;
use crate::pipelines::{monitor_thread, mqtt_client_listen_thread, mqtt_client_publish, mqtt_client_subscribe, mqtt_router_dispatch_thread, reader_configure_thread};
use crate::reception::exchange::collective_perception_message::CollectivePerceptionMessage;
use crate::reception::exchange::cooperative_awareness_message::CooperativeAwarenessMessage;
use crate::reception::exchange::decentralized_environmental_notification_message::DecentralizedEnvironmentalNotificationMessage;
use crate::reception::exchange::map_extended_message::MAPExtendedMessage;
use crate::reception::exchange::signal_phase_and_timing_extended_message::SignalPhaseAndTimingExtendedMessage;
use crate::reception::exchange::Exchange;
use crate::reception::information::Information;
use crate::reception::typed::Typed;
use crate::reception::Reception;

pub async fn run<T: StatefulAnalyzer<C>, C: Send + Sync + 'static>(
    mqtt_host: &str,
    mqtt_port: u16,
    mqtt_client_id: &str,
    mqtt_username: Option<&str>,
    mqtt_password: Option<&str>,
    mqtt_root_topic: &str,
    region_of_responsibility: bool,
    custom_settings: HashMap<String, String>,
    context: Arc<RwLock<C>>,
) {
    loop {
        let topic_list = vec![
            format!("{}/v2x/cam", mqtt_root_topic),
            format!("{}/v2x/cpm", mqtt_root_topic),
            format!("{}/v2x/denm", mqtt_root_topic),
            format!("{}/v2x/map", mqtt_root_topic),
            format!("{}/v2x/spat", mqtt_root_topic),
            format!("{}/info", mqtt_root_topic),
        ];

        let (mut client, event_loop) = Client::new(
            mqtt_host,
            mqtt_port,
            mqtt_client_id,
            mqtt_username,
            mqtt_password,
            None,
            None,
        );

        let configuration = Arc::new(Configuration::new(
            mqtt_client_id.to_string(),
            region_of_responsibility,
            custom_settings.clone(),
        ));

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

        let (load_balancer_handle, mut receivers) = load_balancer_thread(item_receiver);

        // in parallel, analyse exchanges
        let (analyser_item_sender, analyser_item_receiver) = channel();
        let mut analyser_handles = Vec::new();
        analyser_handles.push(
            analyser_generate_thread::<T, C>(configuration.clone(), receivers.remove(0), analyser_item_sender.clone(), context.clone())
        );
        analyser_handles.push(
            analyser_generate_thread::<T, C>(configuration.clone(), receivers.remove(0), analyser_item_sender.clone(), context.clone())
        );
        analyser_handles.push(
            analyser_generate_thread::<T, C>(configuration.clone(), receivers.remove(0), analyser_item_sender.clone(), context.clone())
        );
        analyser_handles.push(
            analyser_generate_thread::<T, C>(configuration.clone(), receivers.remove(0), analyser_item_sender, context.clone())
        );

        // read information
        let reader_configure_handle =
            reader_configure_thread(configuration.clone(), information_receiver);

        // filter exchanges on region of responsibility
        let (publish_item_receiver, publish_monitoring_receiver, filter_handle) =
            filter_thread::<T, C>(configuration.clone(), analyser_item_receiver);

        // in parallel, monitor exchanges publish
        let monitor_publish_handle = monitor_thread(
            "sent_on".to_string(),
            configuration,
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
        for handle in analyser_handles {
            handle.join().unwrap();
        }
        debug!("filter_handle joining...");
        filter_handle.join().unwrap();
        debug!("monitor_publish_handle joining...");
        monitor_publish_handle.join().unwrap();

        warn!("loop done");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn analyser_generate_thread<T: StatefulAnalyzer<C>, C: Send + Sync + 'static>(
    configuration: Arc<Configuration>,
    exchange_receiver: Receiver<Item<Exchange>>,
    analyser_sender: Sender<(Item<Exchange>, Option<Cause>)>,
    context: Arc<RwLock<C>>,
) -> JoinHandle<()> {
    info!("starting analyser generation...");
    let context_clone = context.clone();
    let handle = thread::Builder::new()
        .name("analyser-generator".into())
        .spawn(move || {
            trace!("analyser generation closure entering...");

            let mut analyser = T::new(configuration, context_clone);

            for item in exchange_receiver {
                for publish_item in analyser.analyze(item.clone()) {
                    let cause = Cause::from_exchange(&(item.reception));
                    match analyser_sender.send((publish_item, cause)) {
                        Ok(()) => trace!("analyser sent"),
                        Err(error) => {
                            error!("stopped to send analyser: {}", error);
                            break;
                        }
                    }
                }
                trace!("analyser generation closure finished");
            }
        })
        .unwrap();
    info!("analyser generation started");
    handle
}

fn filter_thread<T: StatefulAnalyzer<C>, C: Sync + 'static>(
    configuration: Arc<Configuration>,
    exchange_receiver: Receiver<(Item<Exchange>, Option<Cause>)>,
) -> (
    Receiver<Item<Exchange>>,
    Receiver<(Item<Exchange>, Option<Cause>)>,
    JoinHandle<()>,
) {
    info!("starting filtering...");
    let (publish_sender, publish_receiver) = channel();
    let (monitoring_sender, monitoring_receiver) = channel();
    let handle = thread::Builder::new()
        .name("filter".into())
        .spawn(move || {
            trace!("filter closure entering...");
            for tuple in exchange_receiver {
                let item = tuple.0;
                let cause = tuple.1;

                //assumed clone, we just send the GeoExtension
                if configuration.is_in_region_of_responsibility(item.topic.geo_extension.clone()) {
                    //assumed clone, we send to 2 channels
                    match publish_sender.send(item.clone()) {
                        Ok(()) => trace!("publish sent"),
                        Err(error) => {
                            error!("stopped to send publish: {}", error);
                            break;
                        }
                    }
                    match monitoring_sender.send((item, cause)) {
                        Ok(()) => trace!("monitoring sent"),
                        Err(error) => {
                            error!("stopped to send monitoring: {}", error);
                            break;
                        }
                    }
                }
                trace!("filter closure finished");
            }
        })
        .unwrap();
    info!("filter started");
    (publish_receiver, monitoring_receiver, handle)
}

fn load_balancer_thread(
    exchange_receiver: Receiver<Item<Exchange>>,
) -> (JoinHandle<()>, Vec<Receiver<Item<Exchange>>>) {
    let mut receivers = Vec::new();
    let (sender_1, receiver_1) = channel();
    let (sender_2, receiver_2) = channel();
    let (sender_3, receiver_3) = channel();
    let (sender_4, receiver_4) = channel();

    receivers.push(receiver_1);
    receivers.push(receiver_2);
    receivers.push(receiver_3);
    receivers.push(receiver_4);

    let handle = thread::Builder::new()
        .name("analyser-generator".into())
        .spawn(move || {
            let mut index = 0;
            let senders: [Sender<Item<Exchange>>; 4] = [
                sender_1,
                sender_2,
                sender_3,
                sender_4,
            ];

            for item in exchange_receiver {
                match senders[index].send(item) {
                    Ok(_) => (),
                    Err(e) => {
                        // TODO
                    }
                }
                index = (index + 1) % 4;
            }
        }
    ).unwrap();

    (handle, receivers)
}
