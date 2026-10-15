use std::sync::Arc;

use cpal::{
    Data, Device, InputCallbackInfo, OutputCallbackInfo, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use nanoaudio_buffer::{Consumer, Producer};
use nanoaudio_core::Configuration;

use crate::{
    DeviceConfiguration, Error, Quality,
    callback::{Capture, Counters, Playback},
    negotiate::{negotiate, preferred, stream_layout},
    resample::Rate,
};

fn resampler(
    quality: Quality,
    configuration: Configuration,
    input_rate: u32,
    output_rate: u32,
) -> Result<Option<Rate>, Error> {
    if input_rate == output_rate {
        return Ok(None);
    }

    Rate::new(quality, configuration.channels, input_rate, output_rate)
        .map(Some)
        .map_err(|_| Error::UnsupportedSampleRate {
            requested: configuration.sample_rate,
            device: if input_rate == configuration.sample_rate {
                output_rate
            } else {
                input_rate
            },
        })
}

/// A playback stream that reads PCM from a ring buffer.
///
/// The audio callback reads the [`Consumer`] given to [`open`](Self::open), resamples the frames when the device runs at another rate, converts them to the device format and channel count, and plays silence when the buffer runs dry. It does not allocate, lock or block. The stream starts paused: fill the buffer, then call [`play`](Self::play). Dropping it stops playback.
pub struct Output {
    stream: Stream,
    configuration: Configuration,
    device: DeviceConfiguration,
    counters: Arc<Counters>,
}

impl Output {
    /// Opens a stream on the default output device, resampling with [`Quality::Sinc`] if needed.
    ///
    /// The ring buffer holds interleaved little-endian frames in `configuration`. Its halves come from a buffer that lives as long as the stream, for example `Box::leak(Box::new(RingBuffer::<N>::new())).split()`.
    ///
    /// When the device cannot run at the requested rate, it runs at the nearest rate it supports, and the callback resamples.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedSampleRate`] when the two rates are too far apart to resample, and [`Error::UnsupportedFormat`] when the device offers no usable configuration.
    pub fn open<const N: usize>(
        consumer: Consumer<'static, N>,
        configuration: Configuration,
    ) -> Result<Self, Error> {
        let device = default_output()?;

        Self::open_device(&device, consumer, configuration, Quality::default())
    }

    /// Opens a stream on `device` like [`open`](Self::open), resampling with `quality` if needed.
    pub fn open_device<const N: usize>(
        device: &Device,
        consumer: Consumer<'static, N>,
        configuration: Configuration,
        quality: Quality,
    ) -> Result<Self, Error> {
        let negotiated = negotiate(device.supported_output_configs()?, configuration)?;
        let resampler = resampler(
            quality,
            configuration,
            configuration.sample_rate,
            negotiated.sample_rate,
        )?;
        let counters = Arc::new(Counters::default());
        let mut playback = Playback::new(
            consumer,
            stream_layout(configuration),
            negotiated.layout(),
            resampler,
            counters.clone(),
        );
        let errors = counters.clone();
        let stream = device.build_output_stream_raw(
            negotiated.stream_config(),
            negotiated.format,
            move |data: &mut Data, _: &OutputCallbackInfo| playback.render(data.bytes_mut()),
            move |_| errors.error(),
            None,
        )?;

        Ok(Self {
            stream,
            configuration,
            device: negotiated,
            counters,
        })
    }

    /// Returns the configuration that matches the default output of `device`, to open a stream without any conversion.
    pub fn preferred_configuration(device: &Device) -> Result<Configuration, Error> {
        Ok(preferred(device.default_output_config()?))
    }

    /// Starts or resumes playback.
    pub fn play(&self) -> Result<(), Error> {
        Ok(self.stream.play()?)
    }

    /// Pauses playback, which some backends do not support.
    pub fn pause(&self) -> Result<(), Error> {
        Ok(self.stream.pause()?)
    }

    /// Returns the configuration of the frames in the ring buffer.
    pub fn configuration(&self) -> Configuration {
        self.configuration
    }

    /// Returns the configuration the device runs with, whose sample rate differs from the stream's when the callback resamples.
    pub fn device_configuration(&self) -> DeviceConfiguration {
        self.device
    }

    /// Returns the number of callbacks that found too few frames and played silence for the rest.
    pub fn underruns(&self) -> u32 {
        self.counters.dropouts()
    }

    /// Returns the number of errors the backend reported, such as device xruns or a lost device.
    pub fn errors(&self) -> u32 {
        self.counters.errors()
    }
}

/// A capture stream that writes PCM into a ring buffer.
///
/// The audio callback converts what the device records to the requested format and channel count, resamples it when the device runs at another rate, and writes it to the [`Producer`] given to [`open`](Self::open). Frames that do not fit are dropped. It does not allocate, lock or block. The stream starts paused until [`play`](Self::play). Dropping it stops capture.
pub struct Input {
    stream: Stream,
    configuration: Configuration,
    device: DeviceConfiguration,
    counters: Arc<Counters>,
}

impl Input {
    /// Opens a stream on the default input device, resampling with [`Quality::Sinc`] if needed.
    ///
    /// The ring buffer receives interleaved little-endian frames in `configuration`. Its halves come from a buffer that lives as long as the stream, for example `Box::leak(Box::new(RingBuffer::<N>::new())).split()`.
    ///
    /// When the device cannot run at the requested rate, it runs at the nearest rate it supports, and the callback resamples.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedSampleRate`] when the two rates are too far apart to resample, and [`Error::UnsupportedFormat`] when the device offers no usable configuration.
    pub fn open<const N: usize>(
        producer: Producer<'static, N>,
        configuration: Configuration,
    ) -> Result<Self, Error> {
        let device = default_input()?;

        Self::open_device(&device, producer, configuration, Quality::default())
    }

    /// Opens a stream on `device` like [`open`](Self::open), resampling with `quality` if needed.
    pub fn open_device<const N: usize>(
        device: &Device,
        producer: Producer<'static, N>,
        configuration: Configuration,
        quality: Quality,
    ) -> Result<Self, Error> {
        let negotiated = negotiate(device.supported_input_configs()?, configuration)?;
        let resampler = resampler(
            quality,
            configuration,
            negotiated.sample_rate,
            configuration.sample_rate,
        )?;
        let counters = Arc::new(Counters::default());
        let mut capture = Capture::new(
            producer,
            negotiated.layout(),
            stream_layout(configuration),
            resampler,
            counters.clone(),
        );
        let errors = counters.clone();
        let stream = device.build_input_stream_raw(
            negotiated.stream_config(),
            negotiated.format,
            move |data: &Data, _: &InputCallbackInfo| capture.capture(data.bytes()),
            move |_| errors.error(),
            None,
        )?;

        Ok(Self {
            stream,
            configuration,
            device: negotiated,
            counters,
        })
    }

    /// Returns the configuration that matches the default input of `device`, to open a stream without any conversion.
    pub fn preferred_configuration(device: &Device) -> Result<Configuration, Error> {
        Ok(preferred(device.default_input_config()?))
    }

    /// Starts or resumes capture.
    pub fn play(&self) -> Result<(), Error> {
        Ok(self.stream.play()?)
    }

    /// Pauses capture, which some backends do not support.
    pub fn pause(&self) -> Result<(), Error> {
        Ok(self.stream.pause()?)
    }

    /// Returns the configuration of the frames written to the ring buffer.
    pub fn configuration(&self) -> Configuration {
        self.configuration
    }

    /// Returns the configuration the device runs with, whose sample rate differs from the stream's when the callback resamples.
    pub fn device_configuration(&self) -> DeviceConfiguration {
        self.device
    }

    /// Returns the number of callbacks that found the ring buffer too full and dropped frames.
    pub fn overruns(&self) -> u32 {
        self.counters.dropouts()
    }

    /// Returns the number of errors the backend reported, such as device xruns or a lost device.
    pub fn errors(&self) -> u32 {
        self.counters.errors()
    }
}

/// Returns the default output device of the default host.
pub fn default_output() -> Result<Device, Error> {
    cpal::default_host()
        .default_output_device()
        .ok_or(Error::NoDevice)
}

/// Returns the default input device of the default host.
pub fn default_input() -> Result<Device, Error> {
    cpal::default_host()
        .default_input_device()
        .ok_or(Error::NoDevice)
}
