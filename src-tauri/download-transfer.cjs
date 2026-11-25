(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  root.ZlackDownloadTransfer = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  const CHUNK_BYTES = 256 * 1024;
  const MAX_DOWNLOAD_BYTES = 256 * 1024 * 1024;

  async function pump(reader, onChunk, maxBytes = MAX_DOWNLOAD_BYTES) {
    let buffer = new Uint8Array(CHUNK_BYTES);
    let buffered = 0;
    let chunkIndex = 0;
    let total = 0;

    try {
      while (true) {
        const result = await reader.read();
        if (result.done) break;
        const value = result.value;
        if (!(value instanceof Uint8Array)) throw new Error("download stream returned invalid bytes");
        total += value.byteLength;
        if (total > maxBytes) throw new Error("file is too large to download");

        let offset = 0;
        while (offset < value.byteLength) {
          const length = Math.min(CHUNK_BYTES - buffered, value.byteLength - offset);
          buffer.set(value.subarray(offset, offset + length), buffered);
          buffered += length;
          offset += length;
          if (buffered === CHUNK_BYTES) {
            await onChunk(buffer, chunkIndex);
            chunkIndex += 1;
            buffer = new Uint8Array(CHUNK_BYTES);
            buffered = 0;
          }
        }
      }

      if (buffered > 0) {
        await onChunk(buffer.subarray(0, buffered), chunkIndex);
        chunkIndex += 1;
      }
      return { total, chunks: chunkIndex };
    } catch (error) {
      await reader.cancel(error);
      throw error;
    }
  }

  return { CHUNK_BYTES, MAX_DOWNLOAD_BYTES, pump };
});
