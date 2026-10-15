globalThis.TextDecoder ??= class {
  decode(bytes) {
    return bytes === undefined ? "" : decodeURIComponent(escape(String.fromCharCode(...bytes)));
  }
};

globalThis.TextEncoder ??= class {
  encode(text = "") {
    return Uint8Array.from(unescape(encodeURIComponent(text)), (character) => character.charCodeAt(0));
  }
};
