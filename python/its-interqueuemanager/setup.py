# Software Name: its-interqueuemanager
# SPDX-FileCopyrightText: Copyright (c) 2023 Orange
# SPDX-License-Identifier: MIT
# Author: Yann E. MORIN <yann.morin@orange.com>

from setuptools import setup, find_packages

setup(
    name="its_iqm",
    version="0.0.0",
    author="Yann E. MORIN",
    author_email="yann(dot)morin(at)orange(dot)com",
    maintainer="Yann E. MORIN",
    maintainer_email="yann(dot)morin(at)orange(dot)com",
    keywords="network, its, vehicle, mqtt, etsi",
    classifiers=[
        "Development Status :: 3 - Alpha",
        "Intended Audience :: Telecommunications Industry",
        "License :: OSI Approved :: MIT License",
        "Topic :: Communications",
        "Topic :: Software Development :: Embedded Systems",
    ],
    url="https://github.com/Orange-OpenSource/its-client",
    packages=find_packages(exclude=["*.tests.*"]),
    include_package_data=True,
    description="The Intelligent Transportation Systems (ITS) [MQTT](https://mqtt.org/) Python packages based on the "
    "[JSon](https://www.json.org) [ETSI](https://www.etsi.org/committee/its) specification transcription.",
    long_description="The Intelligent Transportation Systems (ITS) [MQTT](https://mqtt.org/) Python packages based on "
    "the [JSon](https://www.json.org) [ETSI](https://www.etsi.org/committee/its) specification "
    "transcription. It sends the device status information to an MQTT broker.",
    license="MIT",
    platforms="LINUX",
    install_requires=[
        "paho-mqtt==1.6.1",
    ],
    entry_points={"console_scripts": ["its-iqm = its_iqm.main:main"]},
)
