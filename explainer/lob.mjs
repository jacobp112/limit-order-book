// Thin JavaScript wrapper around the lob-wasm exports. Shared by the
// explainer page and the Node check so both use the same boundary code.

export async function loadEngine(wasmBytes) {
  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const x = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  const readOutput = () => {
    const bytes = new Uint8Array(x.memory.buffer, x.lob_output_ptr(), x.lob_output_len());
    return JSON.parse(decoder.decode(bytes));
  };

  return {
    /** Apply one journal line, e.g. "submit buy limit 100 5". */
    apply(line) {
      const bytes = encoder.encode(line);
      const ptr = x.lob_input(bytes.length);
      new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
      x.lob_apply(bytes.length);
      return readOutput();
    },
    book() {
      x.lob_book();
      return readOutput().book;
    },
    reset() {
      x.lob_reset();
      return readOutput().book;
    },
    stateDigest: () => x.lob_state_digest(),
    eventDigest: () => x.lob_event_digest(),
  };
}
