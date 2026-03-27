#![no_std]
#![no_main]

extern crate alloc;

use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources};
use embassy_time::{Duration, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{AnyPin, Pin};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::interrupt::Priority;
use esp_hal::rng::Rng;
use esp_hal::system::Stack;
use esp_hal::time::Rate;
use esp_hub75::Hub75;
use esp_hub75::Hub75Pins16;
use esp_hub75::framebuffer::compute_frame_count;
use esp_hub75::framebuffer::compute_rows;
use esp_hub75::framebuffer::plain::DmaFrameBuffer;
use esp_println as _;
use esp_radio::wifi::{
    ClientConfig, Config, ModeConfig, WifiController, WifiDevice,
};
use esp_rtos::embassy::InterruptExecutor;
use log::info;
use static_cell::StaticCell;

mod bitmap_store;
mod font;
mod framebuffer;
mod hub75_task;
mod pins;
mod protocol;
mod tcp_server;
mod text_overlay;

use framebuffer::ShadowBuffer;

esp_bootloader_esp_idf::esp_app_desc!();

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: StaticCell<$t> = StaticCell::new();
        STATIC_CELL.uninit().write($val)
    }};
}

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

const ROWS: usize = 64;
const COLS: usize = 128;
const BITS: u8 = 4;
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);

type Hub75Type = Hub75<'static, esp_hal::Async>;
type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;

pub struct Hub75Peripherals<'d> {
    pub lcd_cam: esp_hal::peripherals::LCD_CAM<'d>,
    pub dma_channel: esp_hal::peripherals::DMA_CH0<'d>,
    pub red1: AnyPin<'d>,
    pub grn1: AnyPin<'d>,
    pub blu1: AnyPin<'d>,
    pub red2: AnyPin<'d>,
    pub grn2: AnyPin<'d>,
    pub blu2: AnyPin<'d>,
    pub addr0: AnyPin<'d>,
    pub addr1: AnyPin<'d>,
    pub addr2: AnyPin<'d>,
    pub addr3: AnyPin<'d>,
    pub addr4: AnyPin<'d>,
    pub blank: AnyPin<'d>,
    pub clock: AnyPin<'d>,
    pub latch: AnyPin<'d>,
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger(log::LevelFilter::Info);
    esp_println::println!("TCPScreen-RS booting...");
    info!("TCPScreen-RS starting...");

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Internal SRAM heap (WiFi needs internal RAM for buffers)
    esp_alloc::heap_allocator!(size: 64 * 1024);
    // PSRAM heap for large allocations (bitmaps, etc.) — 2MB available
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);

    esp_rtos::start(timg0.timer0);
    info!("RTOS started");

    // ---- WiFi init ----
    let radio_controller = mk_static!(
        esp_radio::Controller<'static>,
        esp_radio::init().expect("esp_radio init failed")
    );

    let (mut controller, interfaces) = esp_radio::wifi::new(
        radio_controller,
        peripherals.WIFI,
        Config::default(),
    )
    .expect("WiFi init failed");

    let client_config = ClientConfig::default()
        .with_ssid(WIFI_SSID.try_into().expect("SSID too long"))
        .with_password(WIFI_PASSWORD.try_into().expect("Password too long"));
    controller.set_config(&ModeConfig::Client(client_config)).expect("WiFi config failed");

    let wifi_interface = interfaces.sta;

    let net_config = embassy_net::Config::dhcpv4(Default::default());
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    let (stack, runner) = embassy_net::new(
        wifi_interface,
        net_config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(net_task(runner)).ok();
    spawner.spawn(wifi_task(controller)).ok();
    info!("WiFi spawned");

    // ---- HUB75 setup ----
    let hub75_peripherals = Hub75Peripherals {
        lcd_cam: peripherals.LCD_CAM,
        dma_channel: peripherals.DMA_CH0,
        red1: peripherals.GPIO42.degrade(),
        grn1: peripherals.GPIO41.degrade(),
        blu1: peripherals.GPIO40.degrade(),
        red2: peripherals.GPIO38.degrade(),
        grn2: peripherals.GPIO39.degrade(),
        blu2: peripherals.GPIO37.degrade(),
        addr0: peripherals.GPIO45.degrade(),
        addr1: peripherals.GPIO36.degrade(),
        addr2: peripherals.GPIO48.degrade(),
        addr3: peripherals.GPIO35.degrade(),
        addr4: peripherals.GPIO21.degrade(),
        blank: peripherals.GPIO14.degrade(),
        clock: peripherals.GPIO2.degrade(),
        latch: peripherals.GPIO47.degrade(),
    };

    // Large buffers must be in static memory, not on the stack.
    // Use uninit + zero-init to avoid stack overflow from temporary copies.
    static FB_CELL: StaticCell<FBType> = StaticCell::new();
    static SHADOW_CELL: StaticCell<ShadowBuffer> = StaticCell::new();
    let fb = {
        let uninit = FB_CELL.uninit();
        unsafe {
            let ptr = uninit.as_mut_ptr();
            core::ptr::write_bytes(ptr as *mut u8, 0, core::mem::size_of::<FBType>());
            uninit.assume_init_mut()
        }
    };
    let shadow = {
        let uninit = SHADOW_CELL.uninit();
        let s = unsafe {
            let ptr = uninit.as_mut_ptr();
            core::ptr::write_bytes(ptr as *mut u8, 0, core::mem::size_of::<ShadowBuffer>());
            uninit.assume_init_mut()
        };
        s.brightness = 100; // Fix: zero-init set this to 0, must be 100
        s
    };

    // CRITICAL: format() sets up DMA row addressing/timing control bits.
    use esp_hub75::framebuffer::FrameBufferOperations;
    fb.format();

    info!("Framebuffers initialized and formatted");

    info!("Display ready");

    // ---- Start Core 1: HUB75 render ----
    // The hub75_task needs its own framebuffer reference. We use unsafe to create
    // a second reference since the DMA reads are non-destructive and pixel writes
    // from Core 0 are atomic.
    let fb_for_render: &'static mut FBType = unsafe {
        &mut *(fb as *mut FBType)
    };

    let cpu1_fn = move || {
        use esp_rtos::embassy::Executor;

        let hp_executor = mk_static!(
            InterruptExecutor<2>,
            InterruptExecutor::new(sw_ints.software_interrupt2)
        );
        let high_pri_spawner = hp_executor.start(Priority::Priority3);

        high_pri_spawner
            .spawn(hub75_render_task(hub75_peripherals, fb_for_render))
            .ok();

        let lp_executor = mk_static!(Executor, Executor::new());
        lp_executor.run(|_spawner| {});
    };

    const STACK_SIZE: usize = 8192;
    let core1_stack = mk_static!(Stack<STACK_SIZE>, Stack::new());

    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_ints.software_interrupt0,
        sw_ints.software_interrupt1,
        core1_stack,
        cpu1_fn,
    );

    info!("Core 1 started (HUB75 render)");

    // ---- Core 0: wait for IP then run TCP server ----
    info!("Waiting for DHCP...");
    stack.wait_config_up().await;

    if let Some(config) = stack.config_v4() {
        let ip = config.address.address();
        info!("Got IP: {}", ip);
        let ip_str = alloc::format!("{}", ip);
        font::render_text(shadow, fb, 4, 32, 0xFF, 0xFF, 0xFF, 1, &ip_str);
        info!("IP drawn on display");
    }

    info!("Starting TCP server on port 1337");
    tcp_server::tcp_server_task(stack, shadow, fb).await;

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Hub75 render task — runs continuously on Core 1
#[embassy_executor::task]
async fn hub75_render_task(peripherals: Hub75Peripherals<'static>, fb: &'static mut FBType) {
    let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());

    let pins = Hub75Pins16 {
        red1: peripherals.red1,
        grn1: peripherals.grn1,
        blu1: peripherals.blu1,
        red2: peripherals.red2,
        grn2: peripherals.grn2,
        blu2: peripherals.blu2,
        addr0: peripherals.addr0,
        addr1: peripherals.addr1,
        addr2: peripherals.addr2,
        addr3: peripherals.addr3,
        addr4: peripherals.addr4,
        blank: peripherals.blank,
        clock: peripherals.clock,
        latch: peripherals.latch,
    };

    let mut hub75 = Hub75Type::new_async(
        peripherals.lcd_cam,
        pins,
        peripherals.dma_channel,
        tx_descriptors,
        Rate::from_mhz(16),
    )
    .expect("Hub75 init failed");

    info!("HUB75 render task started on Core 1");

    loop {
        let mut xfer = hub75
            .render(fb)
            .map_err(|(e, _)| e)
            .expect("render failed");
        xfer.wait_for_done().await.expect("DMA failed");
        let (result, new_hub75) = xfer.wait();
        hub75 = new_hub75;
        result.expect("transfer failed");
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn wifi_task(mut controller: WifiController<'static>) {
    info!("WiFi: connecting to '{}'", WIFI_SSID);

    loop {
        if !matches!(controller.is_started(), Ok(true)) {
            controller.start_async().await.expect("WiFi start failed");
            info!("WiFi started");
        }

        match controller.connect_async().await {
            Ok(_) => {
                info!("WiFi connected");
                controller.wait_for_event(esp_radio::wifi::WifiEvent::StaDisconnected).await;
                info!("WiFi disconnected, reconnecting...");
            }
            Err(e) => {
                info!("WiFi connect error: {:?}", e);
            }
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}
