registerProcessor("nanoaudio", class extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { module, factory, sampleRate, channels, format } = options.processorOptions;
    this.memory = bindgen.initSync({ module }).memory;
    this.channels = channels;
    this.worklet = bindgen[factory]();
    this.worklet.configure(this.port, sampleRate, channels, format);
    this.port.onmessage = (event) => this.worklet.message(event.data);
  }

  views(pointer, frames) {
    const buffer = this.memory.buffer;
    if (this.buffer !== buffer || this.pointer !== pointer || this.frames !== frames) {
      this.buffer = buffer;
      this.pointer = pointer;
      this.frames = frames;
      this.planes = Array.from({ length: this.channels }, (_, channel) =>
        new Float32Array(buffer, pointer + channel * frames * 4, frames));
    }
    return this.planes;
  }

  process(inputs, outputs) {
    const input = inputs[0];
    const output = outputs[0];
    if (input !== undefined) {
      const frames = output[0].length;
      const planes = this.views(this.worklet.input(frames), frames);
      planes.forEach((plane, channel) => {
        if (channel < input.length) {
          plane.set(input[channel]);
        } else {
          plane.fill(0);
        }
      });
      this.worklet.record(frames);
    } else {
      const frames = output[0].length;
      const planes = this.views(this.worklet.render(frames), frames);
      output.forEach((channel, index) => channel.set(planes[index]));
    }
    return true;
  }
});
