package com.keyboardit.controller;

import static org.junit.Assert.*;

import java.nio.ByteBuffer;
import org.junit.Test;

public class ControllerTest {
  @Test
  public void annexBParameterSetsSupportBothStartCodeLengths() {
    byte[] stream = {0, 0, 0, 1, 0x67, 12, 0, 0, 1, 0x68, 5, 0, 0, 0, 1, 0x65, 9};
    byte[][] parameters = FrameDecoder.parameterSets(stream);
    assertArrayEquals(new byte[] {0, 0, 0, 1, 0x67, 12}, parameters[0]);
    assertArrayEquals(new byte[] {0, 0, 1, 0x68, 5}, parameters[1]);
    assertNull(FrameDecoder.parameterSets(new byte[] {0, 0, 1, 0x65, 1})[0]);
  }

  @Test
  public void protocolInputKeepsSignedCoordinatesAndUsbModifierSemantics() {
    ByteBuffer packet = ByteBuffer.wrap(RemoteInput.move(-12, 300));
    assertEquals(1, packet.get());
    assertEquals(-12, packet.getShort());
    assertEquals(300, packet.getShort());
    assertArrayEquals(new byte[] {0, 0, 0, 6, 0, 1}, RemoteInput.key(6, true, 1));
    assertArrayEquals(new byte[] {2, 0, 0}, RemoteInput.button(0, false));
    assertArrayEquals(new byte[] {3, 0, -127}, RemoteInput.scroll(-1000));
  }
}
