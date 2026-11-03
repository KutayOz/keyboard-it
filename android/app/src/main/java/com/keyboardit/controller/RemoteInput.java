package com.keyboardit.controller;

import android.view.KeyEvent;
import java.nio.ByteBuffer;

final class RemoteInput {
  static byte[] key(int usage, boolean down, int modifiers) {
    return ByteBuffer.allocate(6)
        .put((byte) 0)
        .put((byte) (down ? 0 : 1))
        .putShort((short) usage)
        .putShort((short) modifiers)
        .array();
  }

  static byte[] move(int dx, int dy) {
    return ByteBuffer.allocate(5)
        .put((byte) 1)
        .putShort((short) Math.max(-32767, Math.min(32767, dx)))
        .putShort((short) Math.max(-32767, Math.min(32767, dy)))
        .array();
  }

  static byte[] button(int button, boolean down) {
    return new byte[] {2, (byte) button, (byte) (down ? 1 : 0)};
  }

  static byte[] scroll(int dy) {
    return new byte[] {3, 0, (byte) Math.max(-127, Math.min(127, dy))};
  }

  static int usage(int code) {
    if (code >= KeyEvent.KEYCODE_A && code <= KeyEvent.KEYCODE_Z)
      return 4 + code - KeyEvent.KEYCODE_A;
    if (code >= KeyEvent.KEYCODE_1 && code <= KeyEvent.KEYCODE_9)
      return 30 + code - KeyEvent.KEYCODE_1;
    if (code >= KeyEvent.KEYCODE_F1 && code <= KeyEvent.KEYCODE_F12)
      return 58 + code - KeyEvent.KEYCODE_F1;
    switch (code) {
      case KeyEvent.KEYCODE_0:
        return 39;
      case KeyEvent.KEYCODE_ENTER:
        return 40;
      case KeyEvent.KEYCODE_ESCAPE:
        return 41;
      case KeyEvent.KEYCODE_DEL:
        return 42;
      case KeyEvent.KEYCODE_TAB:
        return 43;
      case KeyEvent.KEYCODE_SPACE:
        return 44;
      case KeyEvent.KEYCODE_MINUS:
        return 45;
      case KeyEvent.KEYCODE_EQUALS:
        return 46;
      case KeyEvent.KEYCODE_LEFT_BRACKET:
        return 47;
      case KeyEvent.KEYCODE_RIGHT_BRACKET:
        return 48;
      case KeyEvent.KEYCODE_BACKSLASH:
        return 49;
      case KeyEvent.KEYCODE_SEMICOLON:
        return 51;
      case KeyEvent.KEYCODE_APOSTROPHE:
        return 52;
      case KeyEvent.KEYCODE_GRAVE:
        return 53;
      case KeyEvent.KEYCODE_COMMA:
        return 54;
      case KeyEvent.KEYCODE_PERIOD:
        return 55;
      case KeyEvent.KEYCODE_SLASH:
        return 56;
      case KeyEvent.KEYCODE_CAPS_LOCK:
        return 57;
      case KeyEvent.KEYCODE_INSERT:
        return 73;
      case KeyEvent.KEYCODE_MOVE_HOME:
        return 74;
      case KeyEvent.KEYCODE_PAGE_UP:
        return 75;
      case KeyEvent.KEYCODE_FORWARD_DEL:
        return 76;
      case KeyEvent.KEYCODE_MOVE_END:
        return 77;
      case KeyEvent.KEYCODE_PAGE_DOWN:
        return 78;
      case KeyEvent.KEYCODE_DPAD_RIGHT:
        return 79;
      case KeyEvent.KEYCODE_DPAD_LEFT:
        return 80;
      case KeyEvent.KEYCODE_DPAD_DOWN:
        return 81;
      case KeyEvent.KEYCODE_DPAD_UP:
        return 82;
      case KeyEvent.KEYCODE_CTRL_LEFT:
        return 224;
      case KeyEvent.KEYCODE_SHIFT_LEFT:
        return 225;
      case KeyEvent.KEYCODE_ALT_LEFT:
        return 226;
      case KeyEvent.KEYCODE_META_LEFT:
        return 227;
      case KeyEvent.KEYCODE_CTRL_RIGHT:
        return 228;
      case KeyEvent.KEYCODE_SHIFT_RIGHT:
        return 229;
      case KeyEvent.KEYCODE_ALT_RIGHT:
        return 230;
      case KeyEvent.KEYCODE_META_RIGHT:
        return 231;
      default:
        return 0;
    }
  }

  static int modifiers(KeyEvent event) {
    int m = 0;
    if (event.isCtrlPressed()) m |= 1;
    if (event.isShiftPressed()) m |= 2;
    if (event.isAltPressed()) m |= 4;
    if (event.isMetaPressed()) m |= 8;
    return m;
  }

  private RemoteInput() {}
}
