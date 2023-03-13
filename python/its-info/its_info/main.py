# Software Name: its-info
# SPDX-FileCopyrightText: Copyright (c) 2022 Orange
# SPDX-License-Identifier: MIT
# Author: Yann E. MORIN <yann.morin@orange.com>

import argparse
import configparser
import json
import linuxfd
import netifaces
import os
import paho.mqtt.client
import time
from its_quadkeys import QuadZone


def main():
    try:
        info = MQTTInfoClient()
        info.loop_forever()
    except Exception as e:
        print(e)
        os._exit(1)


class MQTTInfoClient:
    CFG = "/etc/its/its-info.cfg"
    DEFAULTS = {
        "general": {
            "instance_type": "local",
            "period": 600,
            "dns_ip": None,
            "ntp_host": None,
            "interface": None,
        },
        "mqtt": {
            "host": "127.0.0.1",
            "port": 1883,
            "username": None,
            "password": None,
            "topic": "info",
            "retry": 2,
        },
        "RoR": {
            "type": "none",
            "reload": False,
        },
    }

    def __init__(self):
        parser = argparse.ArgumentParser()
        parser.add_argument(
            "--config",
            "-c",
            default=MQTTInfoClient.CFG,
            help=f"Path to the configuration file (default: {MQTTInfoClient.CFG})",
        )
        args = parser.parse_args()

        self.cfg = configparser.ConfigParser()
        with open(args.config) as f:
            self.cfg.read_file(f)

        # configparser.ConfigParser() only accets strings as values, but we
        # need None for example, so make it a true dict() of dicts()s, which
        # is easier to work with
        self.cfg = {
            s: {k: self.cfg[s][k] for k in self.cfg[s]}
            for s in self.cfg
            if s != "DEFAULT"
        }

        def _set_default(section, key, default):
            if key not in self.cfg[section]:
                self.cfg[section][key] = default

        _set_default("mqtt", "client_id", self.cfg["general"]["instance_id"])
        for s in MQTTInfoClient.DEFAULTS:
            if s not in self.cfg:
                self.cfg[s] = {}
            for k in MQTTInfoClient.DEFAULTS[s]:
                _set_default(s, k, MQTTInfoClient.DEFAULTS[s][k])

        if self.cfg["RoR"]["type"] == "none":
            self.ror = None
        elif self.cfg["RoR"]["type"] == "static":
            self.ror = QuadZone()
            self.ror.load(self.cfg["RoR"]["path"])
            self.ror.optimise()
        else:
            raise RuntimeError(
                f"{self.cfg['RoR']['type']}: unknown or unimplemented RoR type"
            )

        self.timer = linuxfd.timerfd(closeOnExec=True)

        self.client = paho.mqtt.client.Client(client_id=self.cfg["mqtt"]["client_id"])
        self.client.reconnect_delay_set(
            min_delay=1,
            max_delay=self.cfg["mqtt"]["retry"],
        )
        self.client.username_pw_set(
            self.cfg["mqtt"]["username"],
            self.cfg["mqtt"]["password"],
        )
        self.client.on_connect = self.on_connect
        self.client.on_disconnect = self.on_disconnect
        self.client.on_socket_close = self.on_socket_close
        self.client.connect_async(
            host=self.cfg["mqtt"]["host"],
            port=int(self.cfg["mqtt"]["port"]),
        )

    def loop_forever(self):
        self.client.loop_start()
        while True:
            try:
                # We don't care about the number of time the timer has
                # fired that we missed; given the periodicity is in the
                # order of many seconds, it is highly unlikely that we
                # ever miss a tick...
                self.timer.read()
                self.info()
            except InterruptedError:
                # Someone sent a signal to this thread...
                continue
            except KeyboardInterrupt:
                break

        self.client.loop_stop()
        self.client.disconnect()

    def on_connect(self, _client, _userdata, _flags, _rc, _properties=None):
        self.timer.settime(
            value=int(self.cfg["general"]["period"]),
            interval=int(self.cfg["general"]["period"]),
        )
        self.info()

    def on_disconnect(self, _client, _userdata, _rc, _properties=None):
        self.timer.settime(value=0)

    def on_socket_close(self, _client, _userdata, _sock):
        self.timer.settime(value=0)

    def info(self):
        data = {
            "type": "broker",
            "version": "1.2.0",
            "instance_id": self.cfg["general"]["instance_id"],
            "instance_type": self.cfg["general"]["instance_type"],
            "running": True,
            "timestamp": int(1000 * time.time()),
            "validity_duration": int(self.cfg["general"]["period"]) * 2,
        }

        # The IP adresses of the interface may change at runtime
        # so we must grab them every time.
        ips = list()
        if self.cfg["general"]["interface"]:
            ifa = netifaces.ifaddresses(self.cfg["general"]["interface"])
            for familly in [netifaces.AF_INET, netifaces.AF_INET6]:
                if familly in ifa:
                    # IPv6 addresses can look like: 01::EF%iface but we just want 01::EF
                    ips += [i["addr"].split("%")[0] for i in ifa[familly]]

        if self.cfg["general"]["dns_ip"]:
            data["domain_name_servers"] = [self.cfg["general"]["dns_ip"]]
        elif ips:
            data["domain_name_servers"] = ips
        if self.cfg["general"]["ntp_host"]:
            data["ntp_servers"] = [self.cfg["general"]["ntp_host"]]
        elif ips:
            data["ntp_servers"] = ips

        if self.cfg["RoR"]["type"] == "static" and self.cfg["RoR"]["reload"]:
            if self.ror is None:
                self.ror = QuadZone()
            try:
                self.ror.load(self.cfg["RoR"]["path"])
                self.ror.optimise()
            except FileNotFoundError:
                # File not found: consider we have no RoR.
                self.ror = None

        if self.ror is not None:
            data["service_area"] = {"type": "tiles", "quadkeys": sorted(self.ror)}

        self.client.publish(
            topic=self.cfg["mqtt"]["topic"],
            payload=json.dumps(data),
            retain=True,
        )
