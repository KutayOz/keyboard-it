package com.keyboardit.controller;

import java.io.IOException;

final class Native {
  static {
    System.loadLibrary("keyboard_it_mobile");
  }

  static native String identity();

  static native long pairBegin(
      String host, int pairingPort, int desktopPort, String name, String identity)
      throws IOException;

  static native String pairPoll(long handle) throws IOException;

  static native void pairCancel(long handle);

  static native long connect(String target, String identity) throws IOException;

  static native byte[] poll(long handle) throws IOException;

  static native void input(long handle, byte[] input) throws IOException;

  static native void absolute(long handle, int x, int y) throws IOException;

  static native void text(long handle, String text) throws IOException;

  static native void release(long handle) throws IOException;

  static native void close(long handle);

  private Native() {}
}
