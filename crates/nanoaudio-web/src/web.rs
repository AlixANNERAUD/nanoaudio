use std::{
    cell::{Cell, RefCell},
    fmt,
    rc::Rc,
};

use js_sys::{Array, Object, Reflect};
use nanoaudio_buffer::{Consumer, Producer, RingBuffer};
use nanoaudio_core::{Configuration, Format, MAXIMUM_CHANNELS};
use wasm_bindgen::{JsCast, JsError, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AudioContext, AudioContextOptions, AudioWorkletNode, AudioWorkletNodeOptions, Blob,
    BlobPropertyBag, ChannelCountMode, MediaStream, MediaStreamConstraints, MediaStreamTrack,
    MessageEvent, MessagePort, Url,
};

use crate::{Quality, Recorder, Renderer};

/// The size in bytes of the ring buffer between a [`Source`] or a [`Sink`] and the audio stream.
pub const CAPACITY: usize = 16384;

const PROCESSOR: &str = "nanoaudio";
const WORKLET: &str = include_str!("worklet.js");
const POLYFILL: &str = include_str!("polyfill.js");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
    static MODULE_URL: String;
}

/// The errors returned when opening or controlling a stream.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    /// The sample rate is 0, or the channel count is 0 or above [`MAXIMUM_CHANNELS`].
    InvalidConfiguration,
    /// The stream cannot be resampled to or from the rate the audio context runs at.
    ///
    /// This happens when [`Quality::Sinc`] would downsample by more than [`MAXIMUM_RATIO`](crate::MAXIMUM_RATIO).
    UnsupportedSampleRate {
        /// The rate of the stream, in Hz.
        requested: u32,
        /// The rate of the audio context, in Hz.
        device: u32,
    },
    /// A browser API failed, with the JavaScript exception.
    Backend(JsValue),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidConfiguration => write!(
                formatter,
                "the sample rate must not be 0 and the channel count must be between 1 and {MAXIMUM_CHANNELS}"
            ),
            Error::UnsupportedSampleRate { requested, device } => write!(
                formatter,
                "cannot resample between {requested} Hz and the audio context rate of {device} Hz"
            ),
            Error::Backend(error) => write!(formatter, "browser error: {error:?}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<JsValue> for Error {
    fn from(error: JsValue) -> Self {
        Error::Backend(error)
    }
}

impl From<nanoaudio_core::Error> for Error {
    fn from(_: nanoaudio_core::Error) -> Self {
        Error::InvalidConfiguration
    }
}

impl From<Error> for JsValue {
    fn from(error: Error) -> Self {
        match error {
            Error::Backend(error) => error,
            error => JsError::new(&error.to_string()).into(),
        }
    }
}

/// The end of a worklet's message port, to send messages to the page.
///
/// The page receives them in the handler given to [`Output::set_on_message`] or [`Input::set_on_message`].
#[derive(Debug, Clone)]
pub struct Port(MessagePort);

impl Port {
    /// Sends `data` to the page.
    pub fn post(&self, data: &JsValue) -> Result<(), JsValue> {
        self.0.post_message(&Array::of1(data))
    }

    /// Sends `data` to the page, and moves the objects of `transfer` instead of copying them.
    pub fn post_with_transfer(&self, data: &JsValue, transfer: &Array) -> Result<(), JsValue> {
        self.0
            .post_message_with_transferable(&Array::of1(data), transfer)
    }
}

/// Produces the PCM an [`Output`] plays, inside the audio worklet.
pub trait Source: 'static {
    /// Writes interleaved PCM in the format of the stream into `producer`.
    ///
    /// It is called on the audio thread before each render quantum of 128 frames. Whatever is missing for the quantum is played as silence and counted as an underrun, and whatever is left over adds latency.
    fn fill(&mut self, producer: &mut Producer<'_, CAPACITY>, port: &Port);

    /// Handles a message the page sent with [`Output::post`].
    fn message(&mut self, data: JsValue, port: &Port) {
        let _ = (data, port);
    }
}

/// Takes the PCM an [`Input`] captures, inside the audio worklet.
pub trait Sink: 'static {
    /// Reads interleaved PCM in the format of the stream from `consumer`.
    ///
    /// It is called on the audio thread after each render quantum of 128 frames. Frames that no longer fit in the ring buffer are dropped and counted as an overrun.
    fn drain(&mut self, consumer: &mut Consumer<'_, CAPACITY>, port: &Port);

    /// Handles a message the page sent with [`Input::post`].
    fn message(&mut self, data: JsValue, port: &Port) {
        let _ = (data, port);
    }
}

enum Stream {
    Output {
        source: Box<dyn Source>,
        producer: Producer<'static, CAPACITY>,
        consumer: Consumer<'static, CAPACITY>,
        renderer: Option<Box<Renderer>>,
    },
    Input {
        sink: Box<dyn Sink>,
        producer: Producer<'static, CAPACITY>,
        consumer: Consumer<'static, CAPACITY>,
        recorder: Option<Box<Recorder>>,
    },
}

/// The audio side of a stream, living in the audio worklet.
///
/// The application exports a function without arguments that returns one, and passes its JavaScript name to [`Output::open`] or [`Input::open`]. The worklet runs its own instance of the WebAssembly module, which calls that function, so the [`Source`] or [`Sink`] runs on the audio thread and shares no memory with the page.
#[wasm_bindgen]
pub struct Worklet {
    stream: Stream,
    scratch: Vec<f32>,
    port: Option<Port>,
    reported: u32,
}

fn ring() -> (Producer<'static, CAPACITY>, Consumer<'static, CAPACITY>) {
    Box::leak(Box::new(RingBuffer::new())).split()
}

impl Worklet {
    /// Creates the audio side of an [`Output`] that plays what `source` produces.
    ///
    /// The ring buffer is never freed: it lives as long as the audio worklet, which ends with its audio context.
    pub fn output(source: impl Source) -> Self {
        let (producer, consumer) = ring();

        Self::new(Stream::Output {
            source: Box::new(source),
            producer,
            consumer,
            renderer: None,
        })
    }

    /// Creates the audio side of an [`Input`] that hands what it captures to `sink`.
    ///
    /// The ring buffer is never freed: it lives as long as the audio worklet, which ends with its audio context.
    pub fn input(sink: impl Sink) -> Self {
        let (producer, consumer) = ring();

        Self::new(Stream::Input {
            sink: Box::new(sink),
            producer,
            consumer,
            recorder: None,
        })
    }

    fn new(stream: Stream) -> Self {
        Self {
            stream,
            scratch: Vec::new(),
            port: None,
            reported: 0,
        }
    }

    fn report(&mut self, count: u32) {
        if count != self.reported {
            self.reported = count;

            if let Some(port) = &self.port {
                let _ = port.0.post_message(&JsValue::from(count));
            }
        }
    }
}

#[wasm_bindgen]
impl Worklet {
    #[doc(hidden)]
    pub fn configure(
        &mut self,
        port: MessagePort,
        sample_rate: u32,
        channels: u8,
        format: u8,
        context_rate: u32,
        quality: u8,
    ) -> Result<(), JsError> {
        let configuration = Configuration::new(sample_rate, channels, Format::try_from(format)?);
        let quality = Quality::from(quality);

        match &mut self.stream {
            Stream::Output { renderer, .. } => {
                *renderer = Some(Box::new(Renderer::with_rate(
                    configuration,
                    context_rate,
                    quality,
                )?))
            }
            Stream::Input { recorder, .. } => {
                *recorder = Some(Box::new(Recorder::with_rate(
                    configuration,
                    context_rate,
                    quality,
                )?))
            }
        }

        self.port = Some(Port(port));
        Ok(())
    }

    #[doc(hidden)]
    pub fn message(&mut self, data: JsValue) {
        let Some(port) = &self.port else {
            return;
        };

        match &mut self.stream {
            Stream::Output { source, .. } => source.message(data, port),
            Stream::Input { sink, .. } => sink.message(data, port),
        }
    }

    #[doc(hidden)]
    pub fn render(&mut self, frames: usize) -> *const f32 {
        let Stream::Output {
            source,
            producer,
            consumer,
            renderer: Some(renderer),
        } = &mut self.stream
        else {
            return self.scratch.as_ptr();
        };

        if let Some(port) = &self.port {
            source.fill(producer, port);
        }

        let channels = renderer.configuration().channels;
        let length = frames * channels as usize;
        self.scratch.resize(length, 0.0);
        renderer.render(consumer, &mut self.scratch[..length]);

        let underruns = renderer.underruns();
        self.report(underruns);
        self.scratch.as_ptr()
    }

    #[doc(hidden)]
    #[wasm_bindgen(js_name = input)]
    pub fn input_planes(&mut self, frames: usize) -> *mut f32 {
        let channels = match &self.stream {
            Stream::Input {
                recorder: Some(recorder),
                ..
            } => recorder.configuration().channels,
            _ => 0,
        };

        self.scratch.resize(frames * channels as usize, 0.0);
        self.scratch.as_mut_ptr()
    }

    #[doc(hidden)]
    pub fn record(&mut self, frames: usize) {
        let Stream::Input {
            sink,
            producer,
            consumer,
            recorder: Some(recorder),
        } = &mut self.stream
        else {
            return;
        };

        let length = (frames * recorder.configuration().channels as usize).min(self.scratch.len());
        recorder.record(&self.scratch[..length], producer);

        if let Some(port) = &self.port {
            sink.drain(consumer, port);
        }

        let overruns = recorder.overruns();
        self.report(overruns);
    }
}

type Handler = Box<dyn FnMut(JsValue)>;

#[derive(Default)]
struct Shared {
    count: Cell<u32>,
    handler: RefCell<Option<Handler>>,
}

/// Choices for opening a stream, beyond its configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Options {
    /// The resampler used when the audio context runs at another rate than the stream.
    pub quality: Quality,
    /// The rate to ask the audio context for, in Hz, or `None` for the stream's rate.
    ///
    /// The browser may still pick another rate. Asking for the device's own rate, such as 48 kHz, leaves the conversion to the driver instead of the browser.
    pub sample_rate: Option<u32>,
}

struct Node {
    configuration: Configuration,
    device: u32,
    context: AudioContext,
    node: AudioWorkletNode,
    port: MessagePort,
    shared: Rc<Shared>,
    _listener: Closure<dyn FnMut(MessageEvent)>,
}

fn script(parts: &[&str]) -> Result<String, JsValue> {
    let properties = BlobPropertyBag::new();
    properties.set_type("text/javascript");
    let parts: Array = parts.iter().map(|part| JsValue::from_str(part)).collect();
    let blob = Blob::new_with_str_sequence_and_options(&parts, &properties)?;
    Url::create_object_url_with_blob(&blob)
}

fn quote(text: &str) -> Result<String, JsValue> {
    Ok(String::from(js_sys::JSON::stringify(&JsValue::from_str(
        text,
    ))?))
}

impl Node {
    async fn open(
        configuration: Configuration,
        factory: &str,
        input: Option<&MediaStream>,
        options: Options,
    ) -> Result<Self, Error> {
        Renderer::new(configuration)?;

        let context_options = AudioContextOptions::new();
        context_options
            .set_sample_rate(options.sample_rate.unwrap_or(configuration.sample_rate) as f32);
        let context = AudioContext::new_with_context_options(&context_options)?;

        match Self::connect(&context, configuration, factory, input, options.quality).await {
            Ok(node) => Ok(node),
            Err(error) => {
                let _ = context.close();
                Err(error)
            }
        }
    }

    async fn connect(
        context: &AudioContext,
        configuration: Configuration,
        factory: &str,
        input: Option<&MediaStream>,
        quality: Quality,
    ) -> Result<Self, Error> {
        let device = context.sample_rate() as u32;
        let supported = if input.is_some() {
            Recorder::with_rate(configuration, device, quality).map(drop)
        } else {
            Renderer::with_rate(configuration, device, quality).map(drop)
        };

        if supported.is_err() {
            return Err(Error::UnsupportedSampleRate {
                requested: configuration.sample_rate,
                device,
            });
        }

        let polyfill = script(&[POLYFILL])?;
        let module_url = MODULE_URL.with(String::clone);
        let imports = format!(
            "import {};\nimport * as bindgen from {};\n",
            quote(&polyfill)?,
            quote(&module_url)?
        );
        let worklet = script(&[&imports, WORKLET])?;
        let loaded = match context.audio_worklet()?.add_module(&worklet) {
            Ok(promise) => JsFuture::from(promise).await.map(drop),
            Err(error) => Err(error),
        };
        Url::revoke_object_url(&worklet)?;
        Url::revoke_object_url(&polyfill)?;
        loaded?;

        let processor = Object::new();
        Reflect::set(&processor, &"module".into(), &wasm_bindgen::module())?;
        Reflect::set(&processor, &"factory".into(), &factory.into())?;
        Reflect::set(
            &processor,
            &"sampleRate".into(),
            &configuration.sample_rate.into(),
        )?;
        Reflect::set(
            &processor,
            &"channels".into(),
            &configuration.channels.into(),
        )?;
        Reflect::set(
            &processor,
            &"format".into(),
            &u8::from(configuration.format).into(),
        )?;
        Reflect::set(&processor, &"contextRate".into(), &device.into())?;
        Reflect::set(&processor, &"quality".into(), &u8::from(quality).into())?;

        let options = AudioWorkletNodeOptions::new();
        options.set_number_of_inputs(u32::from(input.is_some()));
        options.set_number_of_outputs(1);
        options.set_processor_options(Some(&processor));

        if input.is_some() {
            options.set_output_channel_count(&Array::of1(&1.into()));
            options.set_channel_count(u32::from(configuration.channels));
            options.set_channel_count_mode(ChannelCountMode::Explicit);
        } else {
            options.set_output_channel_count(&Array::of1(&configuration.channels.into()));
        }

        let node = AudioWorkletNode::new_with_options(context, PROCESSOR, &options)?;
        let port = node.port()?;
        let shared = Rc::new(Shared::default());
        let listener = {
            let shared = shared.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let data = event.data();

                if let Some(count) = data.as_f64() {
                    shared.count.set(count as u32);
                } else if let Ok(mut handler) = shared.handler.try_borrow_mut()
                    && let Some(handler) = handler.as_mut()
                {
                    handler(Reflect::get_u32(&data, 0).unwrap_or_default());
                }
            })
        };
        port.set_onmessage(Some(listener.as_ref().unchecked_ref()));

        if let Some(stream) = input {
            context
                .create_media_stream_source(stream)?
                .connect_with_audio_node(&node)?;
        }

        node.connect_with_audio_node(&context.destination())?;

        Ok(Self {
            configuration,
            device,
            context: context.clone(),
            node,
            port,
            shared,
            _listener: listener,
        })
    }

    fn post(&self, data: &JsValue) -> Result<(), Error> {
        Ok(self.port.post_message(data)?)
    }

    fn post_with_transfer(&self, data: &JsValue, transfer: &Array) -> Result<(), Error> {
        Ok(self.port.post_message_with_transferable(data, transfer)?)
    }

    fn set_on_message(&self, handler: impl FnMut(JsValue) + 'static) {
        *self.shared.handler.borrow_mut() = Some(Box::new(handler));
    }

    async fn play(&self) -> Result<(), Error> {
        JsFuture::from(self.context.resume()?).await?;
        Ok(())
    }

    async fn pause(&self) -> Result<(), Error> {
        JsFuture::from(self.context.suspend()?).await?;
        Ok(())
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.port.set_onmessage(None);
        let _ = self.node.disconnect();
        let _ = self.context.close();
    }
}

/// A playback stream: a [`Source`] in the audio worklet writes PCM, and the browser plays it.
///
/// Dropping it closes the audio context.
pub struct Output(Node);

impl Output {
    /// Opens an audio context at the stream's sample rate and starts a worklet built by the exported function named `factory`.
    ///
    /// `factory` is the JavaScript name of a `#[wasm_bindgen]` function without arguments that returns [`Worklet::output`]. The page must be built with `wasm-bindgen --target web`. Browsers start audio only after a user gesture: call it from a click handler, or call [`play`](Self::play) from one. When the browser opens the context at another rate, the worklet resamples with [`Quality::Sinc`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidConfiguration`] for an invalid configuration, [`Error::UnsupportedSampleRate`] when the context rate is too far from the stream's to resample, and [`Error::Backend`] when a Web Audio call fails.
    pub async fn open(configuration: Configuration, factory: &str) -> Result<Self, Error> {
        Self::open_with(configuration, factory, Options::default()).await
    }

    /// Opens a stream like [`open`](Self::open), with `options`.
    pub async fn open_with(
        configuration: Configuration,
        factory: &str,
        options: Options,
    ) -> Result<Self, Error> {
        Ok(Self(
            Node::open(configuration, factory, None, options).await?,
        ))
    }

    /// Returns the configuration of the stream.
    pub fn configuration(&self) -> Configuration {
        self.0.configuration
    }

    /// Returns the sample rate of the audio context in Hz, which differs from the stream's when the worklet resamples.
    pub fn device_sample_rate(&self) -> u32 {
        self.0.device
    }

    /// Returns the number of render quanta that ran out of frames, as last reported by the worklet.
    pub fn underruns(&self) -> u32 {
        self.0.shared.count.get()
    }

    /// Sends `data` to [`Source::message`].
    pub fn post(&self, data: &JsValue) -> Result<(), Error> {
        self.0.post(data)
    }

    /// Sends `data` to [`Source::message`], and moves the objects of `transfer` instead of copying them.
    pub fn post_with_transfer(&self, data: &JsValue, transfer: &Array) -> Result<(), Error> {
        self.0.post_with_transfer(data, transfer)
    }

    /// Calls `handler` with each message the source sends with [`Port::post`].
    pub fn set_on_message(&self, handler: impl FnMut(JsValue) + 'static) {
        self.0.set_on_message(handler);
    }

    /// Starts or resumes the audio context, for example from a user gesture.
    pub async fn play(&self) -> Result<(), Error> {
        self.0.play().await
    }

    /// Suspends the audio context: the worklet stops being called.
    pub async fn pause(&self) -> Result<(), Error> {
        self.0.pause().await
    }

    /// Returns the audio context.
    pub fn context(&self) -> &AudioContext {
        &self.0.context
    }

    /// Returns the worklet node, connected to the destination of the context.
    pub fn node(&self) -> &AudioWorkletNode {
        &self.0.node
    }
}

/// A capture stream: the browser records the microphone, and a [`Sink`] in the audio worklet reads the PCM.
///
/// Dropping it stops the microphone and closes the audio context.
pub struct Input {
    node: Node,
    media: MediaStream,
}

impl Input {
    /// Asks for the microphone, opens an audio context at the stream's sample rate and starts a worklet built by the exported function named `factory`.
    ///
    /// `factory` is the JavaScript name of a `#[wasm_bindgen]` function without arguments that returns [`Worklet::input`]. Echo cancellation, noise suppression and automatic gain are turned off. The browser mixes the microphone to the stream's channel count. When the browser opens the context at another rate, the worklet resamples with [`Quality::Sinc`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidConfiguration`] for an invalid configuration, [`Error::UnsupportedSampleRate`] when the context rate is too far from the stream's to resample, and [`Error::Backend`] when the microphone is refused or a Web Audio call fails.
    pub async fn open(configuration: Configuration, factory: &str) -> Result<Self, Error> {
        Self::open_with(configuration, factory, Options::default()).await
    }

    /// Opens a stream like [`open`](Self::open), with `options`.
    pub async fn open_with(
        configuration: Configuration,
        factory: &str,
        options: Options,
    ) -> Result<Self, Error> {
        Renderer::new(configuration)?;

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let audio = Object::new();
        for constraint in ["echoCancellation", "noiseSuppression", "autoGainControl"] {
            Reflect::set(&audio, &constraint.into(), &false.into())?;
        }
        let constraints = MediaStreamConstraints::new();
        constraints.set_audio(&audio);
        let media: MediaStream = JsFuture::from(
            window
                .navigator()
                .media_devices()?
                .get_user_media_with_constraints(&constraints)?,
        )
        .await?
        .unchecked_into();

        match Node::open(configuration, factory, Some(&media), options).await {
            Ok(node) => Ok(Self { node, media }),
            Err(error) => {
                stop(&media);
                Err(error)
            }
        }
    }

    /// Returns the configuration of the stream.
    pub fn configuration(&self) -> Configuration {
        self.node.configuration
    }

    /// Returns the sample rate of the audio context in Hz, which differs from the stream's when the worklet resamples.
    pub fn device_sample_rate(&self) -> u32 {
        self.node.device
    }

    /// Returns the number of render quanta that dropped frames, as last reported by the worklet.
    pub fn overruns(&self) -> u32 {
        self.node.shared.count.get()
    }

    /// Sends `data` to [`Sink::message`].
    pub fn post(&self, data: &JsValue) -> Result<(), Error> {
        self.node.post(data)
    }

    /// Sends `data` to [`Sink::message`], and moves the objects of `transfer` instead of copying them.
    pub fn post_with_transfer(&self, data: &JsValue, transfer: &Array) -> Result<(), Error> {
        self.node.post_with_transfer(data, transfer)
    }

    /// Calls `handler` with each message the sink sends with [`Port::post`].
    pub fn set_on_message(&self, handler: impl FnMut(JsValue) + 'static) {
        self.node.set_on_message(handler);
    }

    /// Starts or resumes the audio context, for example from a user gesture.
    pub async fn play(&self) -> Result<(), Error> {
        self.node.play().await
    }

    /// Suspends the audio context: the worklet stops being called.
    pub async fn pause(&self) -> Result<(), Error> {
        self.node.pause().await
    }

    /// Returns the audio context.
    pub fn context(&self) -> &AudioContext {
        &self.node.context
    }

    /// Returns the worklet node, fed by the microphone.
    pub fn node(&self) -> &AudioWorkletNode {
        &self.node.node
    }
}

fn stop(media: &MediaStream) {
    for track in media.get_tracks() {
        track.unchecked_into::<MediaStreamTrack>().stop();
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        stop(&self.media);
    }
}
