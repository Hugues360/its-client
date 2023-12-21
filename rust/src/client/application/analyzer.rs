// Software Name: its-client
// SPDX-FileCopyrightText: Copyright (c) 2016-2023 Orange
// SPDX-License-Identifier: MIT License
//
// This software is distributed under the MIT license, see LICENSE.txt file for more details.
//
// Author: Nicolas BUFFON <nicolas.buffon@orange.com> et al.
// Software description: This Intelligent Transportation Systems (ITS) [MQTT](https://mqtt.org/) client based on the [JSon](https://www.json.org) [ETSI](https://www.etsi.org/committee/its) specification transcription provides a ready to connect project for the mobility (connected and autonomous vehicles, road side units, vulnerable road users,...).

use crate::client::configuration::Configuration;

use crate::transport::mqtt::topic::Topic;
use crate::transport::packet::Packet;

use std::sync::{Arc, RwLock};

pub trait Analyzer<T: Topic, C> {
    fn new(configuration: Arc<Configuration>, context: Arc<RwLock<C>>) -> Self
    where
        Self: Sized;

    fn analyze(&mut self, packet: Packet<T>) -> Vec<Packet<T>>;
}
