// Software Name: its-client
// SPDX-FileCopyrightText: Copyright (c) 2016-2023 Orange
// SPDX-License-Identifier: MIT License
//
// This software is distributed under the MIT license, see LICENSE.txt file for more details.
//
// Author: Nicolas BUFFON <nicolas.buffon@orange.com> et al.
// Software description: This Intelligent Transportation Systems (ITS) [MQTT](https://mqtt.org/) client based on the [JSon](https://www.json.org) [ETSI](https://www.etsi.org/committee/its) specification transcription provides a ready to connect project for the mobility (connected and autonomous vehicles, road side units, vulnerable road users,...).

use std::ops::Deref;
use ini::Properties;
use log::info;
use rumqttc::{Key, MqttOptions, TlsConfiguration, Transport};
use crate::client::configuration::configuration_error::ConfigurationError;
use crate::client::configuration::{get_mandatory_field, get_optional_from_section};

pub(crate) mod mqtt_client;
pub(crate) mod mqtt_router;
pub mod topic;

#[cfg(feature = "geo_routing")]
pub mod geo_topic;

pub(crate) fn configure_transport(tls_configuration: Option<TlsConfiguration>, use_websocket: bool, mqtt_options: &mut MqttOptions) {
    if tls_configuration.is_some() {
        let transport = if use_websocket {
            info!("Transport: MQTT over WebSocket; TLS enabled");
            Transport::Wss(tls_configuration.unwrap())
        } else {
            info!("Transport: standard MQTT; TLS enabled");
            Transport::Tls(tls_configuration.unwrap())
        };

        mqtt_options.set_transport(transport);
    } else if use_websocket {
        info!("Transport: MQTT over WebSocket; TLS disabled");
        mqtt_options.set_transport(Transport::Ws);
    } else {
        info!("Transport: standard MQTT; TLS disabled");
    }
}

pub(crate) fn configure_tls(ca_path: &str, alpn: Option<Vec<Vec<u8>>>, client_auth: Option<(Vec<u8>, Key)>) -> TlsConfiguration {
    let ca: Vec<u8> = std::fs::read(ca_path).expect("Failed to read TLS certificate");

    TlsConfiguration::Simple {
        ca,
        alpn,
        client_auth,
    }
}

#[cfg(feature = "native_tls")]
pub(crate) fn configure_native_tls(ca_path: &str, der: Vec<u8>, password: String) -> TlsConfiguration {
    let ca: Vec<u8> = std::fs::read(ca_path).expect("Failed to read TLS certificate");

    TlsConfiguration::SimpleNative {
        ca,
        der,
        password
    }
}


#[cfg(test)]
mod tests {
    use crate::transport::mqtt::configure_tls;

    #[test]
    #[should_panic]
    fn configure_tls_with_invalid_path_should_return_error() {
        let _ = configure_tls("unextisting/path", None, None);
    }

    #[cfg(feature = "native_tls")]
    use crate::transport::mqtt::configure_native_tls;

    #[cfg(feature = "native_tls")]
    #[test]
    #[should_panic]
    fn configure_native_tls_with_invalid_path_should_return_error() {
        let _ = configure_native_tls("unextisting/path", Vec::new(), String::new());
    }
}