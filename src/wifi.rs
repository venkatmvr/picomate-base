// wifi.rs — CYW43439 WiFi subsystem
//
// Owns: CYW43 chip init, embassy-net stack, AP join, DHCP.
// Returns `Wifi` struct with control (for onboard LED) + stack (for future TCP) + IP string.
//
// PIO0 SM0 is reserved for the CYW43 SPI bus — do not use from outside this module.
// Credentials come from .env via build.rs → env!("WIFI_SSID") / env!("WIFI_PASS").

use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::{Config, Stack, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::Peri;
use embassy_time::{Duration, Timer};
use heapless::String;
use static_cell::StaticCell;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");

const FW:  &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
const CLM: &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

bind_interrupts!(struct WifiIrqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

static CYW43_STATE:      StaticCell<cyw43::State>       = StaticCell::new();
static STACK_RESOURCES:  StaticCell<StackResources<4>>  = StaticCell::new();

// ── Tasks ─────────────────────────────────────────────────────────────────────

#[embassy_executor::task]
pub async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

// ── Public handle returned to main ────────────────────────────────────────────

/// Handle returned by `wifi::init`. Carries the CYW43 control (for onboard LED GPIO)
/// and the embassy-net stack (for future TCP sockets / OTA).
pub struct Wifi {
    pub control: cyw43::Control<'static>,
    pub stack:   Stack<'static>,
    pub ip:      String<24>,
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Bring up WiFi end-to-end: chip init → embassy-net → AP join → DHCP.
/// Blocks until an IP address is obtained.
pub async fn init(
    spawner: &Spawner,
    pwr_pin: Peri<'static, PIN_23>,
    cs_pin:  Peri<'static, PIN_25>,
    pio0:    Peri<'static, PIO0>,
    mosi:    Peri<'static, PIN_24>,
    clk:     Peri<'static, PIN_29>,
    dma:     Peri<'static, DMA_CH0>,
) -> Wifi {
    // ── CYW43 chip ────────────────────────────────────────────────────────────
    let pwr = Output::new(pwr_pin, Level::Low);
    let cs  = Output::new(cs_pin,  Level::High);
    let mut pio = Pio::new(pio0, WifiIrqs);
    let spi = PioSpi::new(
        &mut pio.common, pio.sm0, DEFAULT_CLOCK_DIVIDER, pio.irq0,
        cs, mosi, clk, dma,
    );

    let state = CYW43_STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, FW).await;
    spawner.spawn(cyw43_task(runner)).unwrap();
    control.init(CLM).await;
    control.set_power_management(cyw43::PowerManagementMode::None).await;
    info!("cyw43 OK");

    // ── embassy-net stack ─────────────────────────────────────────────────────
    let (stack, net_runner) = embassy_net::new(
        net_device,
        Config::dhcpv4(Default::default()),
        STACK_RESOURCES.init(StackResources::new()),
        0xdead_beef_cafe_babe_u64,
    );
    spawner.spawn(net_task(net_runner)).unwrap();

    // ── Join AP ───────────────────────────────────────────────────────────────
    loop {
        match control.join(WIFI_SSID, JoinOptions::new(WIFI_PASS.as_bytes())).await {
            Ok(()) => break,
            Err(_) => {
                warn!("WiFi join failed, retrying in 2s...");
                Timer::after(Duration::from_secs(2)).await;
            }
        }
    }
    info!("joined {}", WIFI_SSID);

    // ── DHCP ──────────────────────────────────────────────────────────────────
    stack.wait_config_up().await;
    let addr = stack.config_v4().unwrap().address.address();
    let o = addr.octets();
    let mut ip: String<24> = String::new();
    core::fmt::write(&mut ip, format_args!("{}.{}.{}.{}", o[0], o[1], o[2], o[3])).ok();
    info!("IP: {}", ip.as_str());

    Wifi { control, stack, ip }
}
