package com.keyboardit.controller;

import android.media.MediaCodec;
import android.media.MediaFormat;
import android.view.Surface;
import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;

/** Ordered, bounded H.264 access units; reference frames are never dropped. */
final class FrameDecoder implements AutoCloseable {
  interface Listener {
    void ready();

    void failed(String reason);
  }

  private final ArrayBlockingQueue<byte[]> frames = new ArrayBlockingQueue<>(3);
  private volatile boolean running = true;
  private final Thread thread;

  FrameDecoder(int width, int height, Surface surface, Listener listener) {
    thread = new Thread(() -> decode(width, height, surface, listener), "keyboard-it-decoder");
    thread.start();
  }

  boolean offer(byte[] annexB) {
    return running && frames.offer(annexB);
  }

  private void decode(int width, int height, Surface surface, Listener listener) {
    MediaCodec codec = null;
    try {
      byte[] first = null;
      while (running && first == null) first = frames.poll(100, TimeUnit.MILLISECONDS);
      if (!running) return;
      MediaFormat format = MediaFormat.createVideoFormat("video/avc", width, height);
      format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 4 * 1024 * 1024);
      byte[][] config = parameterSets(first);
      if (config[0] == null || config[1] == null)
        throw new IllegalStateException("H.264 anahtar karesi SPS/PPS içermiyor");
      format.setByteBuffer("csd-0", ByteBuffer.wrap(config[0]));
      format.setByteBuffer("csd-1", ByteBuffer.wrap(config[1]));
      codec = MediaCodec.createDecoderByType("video/avc");
      codec.configure(format, surface, null, 0);
      codec.start();
      MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
      long pts = 0;
      boolean shown = false;
      byte[] frame = first;
      while (running) {
        if (frame == null) frame = frames.poll(10, TimeUnit.MILLISECONDS);
        if (frame != null) {
          int index = codec.dequeueInputBuffer(10000);
          if (index >= 0) {
            ByteBuffer buffer = codec.getInputBuffer(index);
            if (buffer == null || frame.length > buffer.capacity())
              throw new IllegalStateException("Video karesi çözücü sınırını aştı");
            buffer.clear();
            buffer.put(frame);
            codec.queueInputBuffer(index, 0, frame.length, pts, 0);
            pts += 33333;
            frame = null;
          }
        }
        int output;
        while ((output = codec.dequeueOutputBuffer(info, 0)) >= 0) {
          codec.releaseOutputBuffer(output, true);
          if (!shown) {
            shown = true;
            listener.ready();
          }
        }
      }
    } catch (Exception error) {
      if (running) listener.failed("Video çözülemedi: " + error.getClass().getSimpleName());
    } finally {
      running = false;
      if (codec != null) {
        try {
          codec.stop();
        } catch (RuntimeException ignored) {
        }
        codec.release();
      }
      frames.clear();
    }
  }

  static byte[][] parameterSets(byte[] data) {
    byte[][] result = new byte[2][];
    int start = -1, payload = -1;
    for (int i = 0; i <= data.length; i++) {
      int length = 0;
      if (i + 3 <= data.length && data[i] == 0 && data[i + 1] == 0) {
        if (data[i + 2] == 1) length = 3;
        else if (i + 4 <= data.length && data[i + 2] == 0 && data[i + 3] == 1) length = 4;
      }
      if (length > 0 || i == data.length) {
        if (start >= 0 && payload < i) {
          int type = data[payload] & 31;
          if (type == 7 || type == 8) result[type - 7] = Arrays.copyOfRange(data, start, i);
        }
        if (length > 0) {
          start = i;
          payload = i + length;
          i += length - 1;
        }
      }
    }
    return result;
  }

  public void close() {
    running = false;
    thread.interrupt();
    frames.clear();
  }
}
