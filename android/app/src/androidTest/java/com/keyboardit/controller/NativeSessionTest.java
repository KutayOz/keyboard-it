package com.keyboardit.controller;

import static org.junit.Assert.*;

import android.content.Context;
import android.graphics.SurfaceTexture;
import android.view.Surface;
import androidx.test.platform.app.InstrumentationRegistry;
import java.nio.ByteBuffer;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import org.json.JSONObject;
import org.junit.Test;

/** Real JNI -> Rust Noise -> loopback fixture -> Android hardware/software codec. */
public class NativeSessionTest {
  private String identity;

  private JSONObject pair(int pairingPort, int desktopPort) throws Exception {
    if (identity == null)
      identity =
          new DeviceStore(InstrumentationRegistry.getInstrumentation().getTargetContext())
              .identity();
    long operation =
        Native.pairBegin("10.0.2.2", pairingPort, desktopPort, "Android emulator", identity);
    try {
      long deadline = System.currentTimeMillis() + 6000;
      boolean fingerprint = false;
      while (System.currentTimeMillis() < deadline) {
        String result = Native.pairPoll(operation);
        if (result == null) continue;
        JSONObject event = new JSONObject(result);
        if (event.getString("type").equals("fingerprint")) {
          assertEquals(8, event.getString("code").split(" ").length);
          fingerprint = true;
        } else if (event.getString("type").equals("paired")) {
          assertTrue(fingerprint);
          return event.getJSONObject("target");
        } else fail("Pairing rejected");
      }
      throw new AssertionError("Pairing timed out");
    } finally {
      Native.pairCancel(operation);
    }
  }

  private void status(long session, String expected) throws Exception {
    long deadline = System.currentTimeMillis() + 5000;
    while (System.currentTimeMillis() < deadline) {
      byte[] packet = Native.poll(session);
      if (packet == null) continue;
      if (packet[0] == 3
          && new String(packet, 1, packet.length - 1, java.nio.charset.StandardCharsets.UTF_8)
              .equals(expected)) return;
      if (packet[0] == 0) fail("Disconnected unexpectedly");
    }
    fail("Fixture did not acknowledge " + expected);
  }

  @Test
  public void screenlessMacReceivesInputAndUnicodeAndCredentialsAreEncrypted() throws Exception {
    JSONObject target = pair(57821, 57822);
    Context context = InstrumentationRegistry.getInstrumentation().getTargetContext();
    DeviceStore store = new DeviceStore(context);
    store.remember(target);
    assertEquals(target.getString("secret"), store.load().getJSONObject(0).getString("secret"));
    String encrypted =
        context.getSharedPreferences("devices", Context.MODE_PRIVATE).getString("ciphertext", "");
    assertFalse(encrypted.contains(target.getString("secret")));
    long session = Native.connect(target.toString(), identity);
    try {
      byte[] caps = Native.poll(session);
      assertArrayEquals(new byte[] {4, 0, 1, 1}, caps);
      Native.input(session, RemoteInput.move(7, -2));
      status(session, "fixture:input-ok");
      Native.text(session, "Türkçe 👋");
      status(session, "fixture:unicode-ok");
      Native.release(session);
      status(session, "fixture:release-ok");
    } finally {
      Native.close(session);
    }
    context
        .getSharedPreferences("devices", Context.MODE_PRIVATE)
        .edit()
        .putString("ciphertext", encrypted.substring(0, encrypted.length() - 4) + "AAAA")
        .commit();
    try {
      store.load();
      fail("Tampered registry was accepted");
    } catch (Exception expected) {
    }
    context
        .getSharedPreferences("devices", Context.MODE_PRIVATE)
        .edit()
        .putString("ciphertext", encrypted)
        .commit();
    store.forget(target.getString("device_id"));
  }

  @Test
  public void windowsSessionDecodesARealH264Keyframe() throws Exception {
    JSONObject target = pair(57811, 57812);
    long session = Native.connect(target.toString(), identity);
    SurfaceTexture texture = new SurfaceTexture(false);
    texture.setDefaultBufferSize(320, 180);
    Surface surface = new Surface(texture);
    FrameDecoder decoder = null;
    CountDownLatch rendered = new CountDownLatch(1);
    AtomicReference<String> error = new AtomicReference<>();
    try {
      long deadline = System.currentTimeMillis() + 6000;
      while (System.currentTimeMillis() < deadline) {
        byte[] packet = Native.poll(session);
        if (packet == null) continue;
        if (packet[0] == 1) {
          ByteBuffer d = ByteBuffer.wrap(packet);
          d.get();
          assertEquals(320, d.getInt());
          assertEquals(180, d.getInt());
          decoder =
              new FrameDecoder(
                  320,
                  180,
                  surface,
                  new FrameDecoder.Listener() {
                    public void ready() {
                      rendered.countDown();
                    }

                    public void failed(String reason) {
                      error.set(reason);
                      rendered.countDown();
                    }
                  });
        }
        if (packet[0] == 2) {
          assertNotNull(decoder);
          assertTrue(decoder.offer(java.util.Arrays.copyOfRange(packet, 2, packet.length)));
          break;
        }
        if (packet[0] == 0) fail("Disconnected before video");
      }
      assertTrue("Decoder produced no output", rendered.await(6, TimeUnit.SECONDS));
      assertNull(error.get());
      Native.absolute(session, 32767, 32767);
      status(session, "fixture:input-ok");
    } finally {
      Native.close(session);
      if (decoder != null) decoder.close();
      surface.release();
      texture.release();
    }
  }

  @Test
  public void malformedAndClosedNativeHandlesFailWithoutCrashing() throws Exception {
    try {
      Native.input(999999, new byte[] {99});
      fail("Closed session accepted");
    } catch (java.io.IOException expected) {
    }
    try {
      Native.connect("{}", Native.identity());
      fail("Invalid saved target accepted");
    } catch (java.io.IOException expected) {
    }
  }

  @Test
  public void nativeActivityShowsVideoDispatchesTouchAndClosesOnPause() throws Exception {
    android.app.Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
    Context context = instrumentation.getTargetContext();
    JSONObject target = pair(57811, 57812);
    DeviceStore store = new DeviceStore(context);
    store.remember(target);
    MainActivity activity =
        (MainActivity)
            instrumentation.startActivitySync(
                new android.content.Intent(context, MainActivity.class)
                    .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK));
    try {
      instrumentation.runOnMainSync(
          () -> {
            android.widget.Button connect =
                findButton(activity.findViewById(android.R.id.content), "fixture-windows");
            assertNotNull(connect);
            connect.performClick();
          });
      java.lang.reflect.Field ready = MainActivity.class.getDeclaredField("ready");
      ready.setAccessible(true);
      long deadline = System.currentTimeMillis() + 8000;
      while (!ready.getBoolean(activity) && System.currentTimeMillis() < deadline) {
        // Video/native callbacks need not make the whole Android message queue
        // idle. Wait on the actual volatile readiness condition with a deadline.
        Thread.sleep(40);
      }
      assertTrue("Activity never presented a decoded frame", ready.getBoolean(activity));
      java.lang.reflect.Field surfaceField = MainActivity.class.getDeclaredField("surface");
      surfaceField.setAccessible(true);
      android.view.View surfaceView = (android.view.View) surfaceField.get(activity);
      instrumentation.runOnMainSync(
          () -> {
            float x = surfaceView.getWidth() / 2f, y = surfaceView.getHeight() / 2f;
            long now = android.os.SystemClock.uptimeMillis();
            android.view.MotionEvent down = android.view.MotionEvent.obtain(now, now, 0, x, y, 0),
                up = android.view.MotionEvent.obtain(now, now + 40, 1, x, y, 0);
            surfaceView.dispatchTouchEvent(down);
            surfaceView.dispatchTouchEvent(up);
            down.recycle();
            up.recycle();
          });
      java.lang.reflect.Field statusField = MainActivity.class.getDeclaredField("status");
      statusField.setAccessible(true);
      deadline = System.currentTimeMillis() + 4000;
      while (!((android.widget.TextView) statusField.get(activity))
              .getText()
              .toString()
              .contains("fixture:input-ok")
          && System.currentTimeMillis() < deadline) {
        Thread.sleep(30);
      }
      assertTrue(
          "Touch did not reach authenticated host",
          ((android.widget.TextView) statusField.get(activity))
              .getText()
              .toString()
              .contains("fixture:input-ok"));
      java.lang.reflect.Field sessionField = MainActivity.class.getDeclaredField("session");
      sessionField.setAccessible(true);
      long handle = ((java.util.concurrent.atomic.AtomicLong) sessionField.get(activity)).get();
      instrumentation.runOnMainSync(activity::finish);
      long closedDeadline = System.currentTimeMillis() + 3000;
      while (((java.util.concurrent.atomic.AtomicLong) sessionField.get(activity)).get() != 0
          && System.currentTimeMillis() < closedDeadline) Thread.sleep(20);
      assertEquals(0, ((java.util.concurrent.atomic.AtomicLong) sessionField.get(activity)).get());
      try {
        Native.input(handle, RemoteInput.move(1, 1));
        fail("Background activity retained input authority");
      } catch (java.io.IOException expected) {
      }
    } finally {
      instrumentation.runOnMainSync(activity::finish);
      store.forget(target.getString("device_id"));
    }
  }

  private android.widget.Button findButton(android.view.View view, String name) {
    if (view instanceof android.widget.Button
        && ((android.widget.Button) view).getText().toString().contains(name))
      return (android.widget.Button) view;
    if (view instanceof android.view.ViewGroup) {
      android.view.ViewGroup group = (android.view.ViewGroup) view;
      for (int i = 0; i < group.getChildCount(); i++) {
        android.widget.Button found = findButton(group.getChildAt(i), name);
        if (found != null) return found;
      }
    }
    return null;
  }

  @Test
  public void androidNsdResolvesPairingV2ServiceAndStopsCleanly() throws Exception {
    android.app.Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
    Context context = instrumentation.getTargetContext();
    android.net.nsd.NsdManager manager =
        (android.net.nsd.NsdManager) context.getSystemService(Context.NSD_SERVICE);
    android.net.nsd.NsdServiceInfo info = new android.net.nsd.NsdServiceInfo();
    String name = "keyboard-it-test-" + java.util.UUID.randomUUID().toString().substring(0, 8);
    info.setServiceName(name);
    info.setServiceType("_keyboard-it._tcp.");
    info.setPort(57811);
    info.setAttribute("pairv", "2");
    info.setAttribute("pair", "57811");
    info.setAttribute("desktop_port", "57812");
    info.setAttribute("device_id", "fixture-discovery");
    CountDownLatch registered = new CountDownLatch(1), resolved = new CountDownLatch(1);
    AtomicReference<String> failure = new AtomicReference<>();
    AtomicReference<Discovery> browser = new AtomicReference<>();
    android.net.nsd.NsdManager.RegistrationListener registration =
        new android.net.nsd.NsdManager.RegistrationListener() {
          public void onServiceRegistered(android.net.nsd.NsdServiceInfo i) {
            registered.countDown();
          }

          public void onRegistrationFailed(android.net.nsd.NsdServiceInfo i, int e) {
            failure.set("registration:" + e);
            registered.countDown();
          }

          public void onServiceUnregistered(android.net.nsd.NsdServiceInfo i) {}

          public void onUnregistrationFailed(android.net.nsd.NsdServiceInfo i, int e) {}
        };
    manager.registerService(info, android.net.nsd.NsdManager.PROTOCOL_DNS_SD, registration);
    try {
      assertTrue(registered.await(5, TimeUnit.SECONDS));
      assertNull(failure.get());
      instrumentation.runOnMainSync(
          () -> {
            Discovery d =
                new Discovery(
                    context,
                    new Discovery.Listener() {
                      public void changed(java.util.Map<String, Discovery.Peer> peers) {
                        for (Discovery.Peer p : peers.values())
                          if (p.identity.equals("fixture-discovery")
                              && p.pairingPort == 57811
                              && p.desktopPort == 57812) resolved.countDown();
                      }

                      public void error(String reason) {
                        failure.set(reason);
                        resolved.countDown();
                      }
                    });
            browser.set(d);
            d.start();
          });
      assertTrue(
          "NSD failed to resolve its advertised fixture", resolved.await(12, TimeUnit.SECONDS));
      assertNull(failure.get());
    } finally {
      if (browser.get() != null) instrumentation.runOnMainSync(() -> browser.get().stop());
      manager.unregisterService(registration);
    }
  }
}
