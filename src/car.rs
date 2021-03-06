#![allow(unused_imports)]
#![allow(clippy::single_component_path_imports)]

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::{cell::RefCell, env, sync::atomic::*, sync::Arc, thread, time::*};

use anyhow::bail;

use embedded_svc::mqtt::client::utils::ConnState;
use log::*;

use url;

use smol;

use embedded_hal::adc::OneShot;
use embedded_hal::blocking::delay::DelayMs;
use embedded_hal::digital::v2::{OutputPin, PinState};

use embedded_svc::eth;
use embedded_svc::eth::{Eth, TransitionalState};
use embedded_svc::httpd::registry::*;
use embedded_svc::httpd::*;
use embedded_svc::io;
use embedded_svc::ipv4;
use embedded_svc::mqtt::client::{Client, Connection, MessageImpl, Publish, QoS};
use embedded_svc::ping::Ping;
use embedded_svc::sys_time::SystemTime;
use embedded_svc::timer::TimerService;
use embedded_svc::timer::*;
use embedded_svc::wifi::*;

use esp_idf_svc::eth::*;
use esp_idf_svc::eventloop::*;
use esp_idf_svc::eventloop::*;
use esp_idf_svc::httpd as idf;
use esp_idf_svc::httpd::ServerRegistry;
use esp_idf_svc::mqtt::client::*;
use esp_idf_svc::netif::*;
use esp_idf_svc::nvs::*;
use esp_idf_svc::ping;
use esp_idf_svc::sntp;
use esp_idf_svc::sysloop::*;
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::timer::*;
use esp_idf_svc::wifi::*;

use esp_idf_hal::adc;
use esp_idf_hal::delay;
use esp_idf_hal::gpio;
use esp_idf_hal::i2c;
use esp_idf_hal::prelude::*;
use esp_idf_hal::spi;

use esp_idf_sys::{self, c_types};
use esp_idf_sys::{esp, EspError};

use display_interface_spi::SPIInterfaceNoCS;

use embedded_graphics::mono_font::{ascii::FONT_10X20, MonoTextStyle};
use embedded_graphics::pixelcolor::*;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::*;
use embedded_graphics::text::*;

use ili9341;
use ssd1306;
use ssd1306::mode::DisplayConfig;
use st7789;

use epd_waveshare::{epd4in2::*, graphics::VarDisplay, prelude::*};

use esp_idf_hal::ledc::*;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::prelude::*;
use std::{borrow::Borrow, time::Duration};
use esp_idf_hal::ledc;

use pid::Pid;

use arc_swap::{ArcSwap, AsRaw};

// reference https://github.com/esp-rs/esp-idf-hal/blob/447fcc3616e3a3643ca109d4bc7acf40754da9af/examples/ledc-threads.rs

struct EnginePWMChannel<C0, H0, T0, P0, C1, H1, T1, P1> where
    C0: HwChannel,
    H0: HwTimer,
    T0: Borrow<ledc::Timer<H0>>,
    P0: gpio::OutputPin,
    C1: HwChannel,
    H1: HwTimer,
    T1: Borrow<ledc::Timer<H1>>,
    P1: gpio::OutputPin,
{
    positive: Channel<C0, H0, T0, P0>,
    negative: Channel<C1, H1, T1, P1>,
}

type DutyUnsigned = u32;
type DutySigned = i32;

impl<C0, H0, T0, P0, C1, H1, T1, P1> EnginePWMChannel<C0, H0, T0, P0, C1, H1, T1, P1> where
    C0: HwChannel,
    H0: HwTimer,
    T0: Borrow<ledc::Timer<H0>>,
    P0: gpio::OutputPin,
    C1: HwChannel,
    H1: HwTimer,
    T1: Borrow<ledc::Timer<H1>>,
    P1: gpio::OutputPin,
{
    fn get_max_duty_unsigned(&self) -> DutyUnsigned {
        assert_eq!(self.positive.get_max_duty(), self.negative.get_max_duty());
        self.positive.get_max_duty()
    }
    fn set_duty(&mut self, duty: DutySigned) -> Result<()> {
        if duty > 0 {
            self.positive.set_duty(duty as DutyUnsigned)?;
            self.negative.set_duty(0)?;
        } else {
            self.positive.set_duty(0)?;
            self.negative.set_duty(-duty as DutyUnsigned)?;
        }
        Ok(())
    }
}

struct CarEngines<C0, H0, T0, P0, C1, H1, T1, P1, C2, H2, T2, P2, C3, H3, T3, P3> where
    C0: HwChannel,
    H0: HwTimer,
    T0: Borrow<ledc::Timer<H0>>,
    P0: gpio::OutputPin,
    C1: HwChannel,
    H1: HwTimer,
    T1: Borrow<ledc::Timer<H1>>,
    P1: gpio::OutputPin,
    C2: HwChannel,
    H2: HwTimer,
    T2: Borrow<ledc::Timer<H2>>,
    P2: gpio::OutputPin,
    C3: HwChannel,
    H3: HwTimer,
    T3: Borrow<ledc::Timer<H3>>,
    P3: gpio::OutputPin,
{
    engine1: EnginePWMChannel<C0, H0, T0, P0, C1, H1, T1, P1>,
    engine2: EnginePWMChannel<C2, H2, T2, P2, C3, H3, T3, P3>,
}

impl<C0, H0, T0, P0, C1, H1, T1, P1, C2, H2, T2, P2, C3, H3, T3, P3>
CarEngines<C0, H0, T0, P0, C1, H1, T1, P1, C2, H2, T2, P2, C3, H3, T3, P3> where
    C0: HwChannel,
    H0: HwTimer,
    T0: Borrow<ledc::Timer<H0>>,
    P0: gpio::OutputPin,
    C1: HwChannel,
    H1: HwTimer,
    T1: Borrow<ledc::Timer<H1>>,
    P1: gpio::OutputPin,
    C2: HwChannel,
    H2: HwTimer,
    T2: Borrow<ledc::Timer<H2>>,
    P2: gpio::OutputPin,
    C3: HwChannel,
    H3: HwTimer,
    T3: Borrow<ledc::Timer<H3>>,
    P3: gpio::OutputPin,
{
    fn get_max_duty_unsigned(&self) -> DutyUnsigned {
        assert_eq!(self.engine1.get_max_duty_unsigned(), self.engine2.get_max_duty_unsigned());
        self.engine1.get_max_duty_unsigned()
    }
    fn set_duty_same(&mut self, duty: DutySigned) -> Result<()> {
        self.engine1.set_duty(duty)?;
        self.engine2.set_duty(duty)?;
        Ok(())
    }
}

fn recv_client_thread<CB: FnMut(common::ControlData) -> Result<()>, Cont: Fn() -> bool>(cont: Cont, mut cb: CB) -> Result<()> {
    info!("About to open a TCP connection to 192.168.71.1 port 8080");



    let mut stream = loop {
        match TcpStream::connect("192.168.71.1:8080") {
            Ok(stream) => break stream,
            Err(e) => {
                warn!("Failed to connect to server: {:?}", e);
                std::thread::sleep(Duration::from_millis(1000));
            }
        }
    };

    let mut buffer = vec![0; common::ControlData::size()];

    loop {
        if !cont() { break; }
        stream.read_exact(&mut buffer)?;
        if !cont() { break; }
        cb(*common::ControlData::from_slice(&buffer))?;
    }

    Ok(())
}

#[derive(PartialEq)]
enum State {
    Init,
    ForwardToLine,
    Done,
}

const BEEP_HALF_CYCLE: Duration = Duration::from_millis(200);

fn main() -> Result<()> {
    esp_idf_sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();

    let mut wifi = common::init_wifi_client()?;


    let config = config::TimerConfig::default().frequency(25.kHz().into());
    let timer = Arc::new(ledc::Timer::new(peripherals.ledc.timer0, &config)?);

    let mut car_engines = CarEngines {
        engine1: EnginePWMChannel {
            positive: Channel::new(peripherals.ledc.channel0, timer.clone(), peripherals.pins.gpio4)?,
            negative: Channel::new(peripherals.ledc.channel1, timer.clone(), peripherals.pins.gpio5)?,
        },
        engine2: EnginePWMChannel {
            positive: Channel::new(peripherals.ledc.channel2, timer.clone(), peripherals.pins.gpio6)?,
            negative: Channel::new(peripherals.ledc.channel3, timer.clone(), peripherals.pins.gpio7)?,
        },
    };
    let mut car_beep = peripherals.pins.gpio0.into_output()?;

    // disable = low enable = high
    let beep_disable_val = PinState::Low;
    let beep_enable_val = PinState::High;


    let max_duty = car_engines.get_max_duty_unsigned();

    let mut pid: Pid<f64> = Pid::new(10.0, 0.0, 0.0, 100.0, 100.0, 100.0, 1000.0, 0.0);

    let control: Arc<ArcSwap<_>> = Arc::new(ArcSwap::from(Arc::new(common::ControlData::empty())));

    let state: Arc<ArcSwap<_>> = Arc::new(ArcSwap::from(Arc::new(State::Init)));

    let mut children = vec![];

    println!("Rust main thread: {:?}", thread::current());

    {
        let control = control.clone();
        let state = state.clone();
        let state0 = state.clone();
        children.push(thread::spawn(move || recv_client_thread(
            move || { **state0.load() != State::Done },
            move |data| {
                println!("Got data {:?}", data);
                control.store(Arc::new(data));
                let load = state.load();
                let current = load.as_raw();
                if **load == State::Init {
                    drop(load);
                    state.compare_and_swap(current, Arc::new(State::ForwardToLine));
                }
                Ok(())
            }).unwrap()));
    }

    {
        let control = control.clone();
        let state = state.clone();
        let mut task = move || -> Result<()> {
            match **state.load() {
                State::Init => {
                    car_engines.engine1.set_duty(0)?;
                    car_engines.engine2.set_duty(0)?;
                }
                State::ForwardToLine => {
                    let output = pid.next_control_output(control.load().offset as f64).output;
                    let duty = ((output as f64 / 1000.0) * (max_duty as f64)) as DutySigned;
                    car_engines.engine1.set_duty(duty)?;
                    car_engines.engine2.set_duty(duty)?;
                    // todo: alternative control a little bit in case the link is slow
                }
                State::Done => {
                    car_engines.engine1.set_duty(0)?;
                    car_engines.engine2.set_duty(0)?;
                    //main_timer.cancel();
                }
            }
            Ok(())
        };
        task()?;
        let mut engines_timer = EspTimerService::new()?.timer(move || task().unwrap())?;
        // 0.1s
        engines_timer.every(Duration::from_millis(100))?;
    }

    {
        let state = state.clone();
        let mut beeping = beep_disable_val;
        let mut task = move || -> Result<()> {
            match **state.load() {
                State::Init => {
                    car_beep.set_state(beep_disable_val)?;
                }
                State::ForwardToLine => {
                    car_beep.set_state(beeping)?;
                    beeping = !beeping;
                }
                State::Done => {
                    car_beep.set_state(beep_enable_val)?;
                }
            }
            Ok(())
        };
        task()?;
        let mut beep_timer = EspTimerService::new()?.timer(move || task().unwrap())?;
        beep_timer.every(BEEP_HALF_CYCLE)?;
    }

    for child in children {
        // Wait for the thread to finish. Returns a result.
        let _ = child.join();
    }

    Ok(())
}

