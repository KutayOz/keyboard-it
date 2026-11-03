package com.keyboardit.controller;

import android.app.Activity;
import android.app.AlertDialog;
import android.graphics.Color;
import android.graphics.Typeface;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.text.InputFilter;
import android.view.GestureDetector;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.*;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import org.json.JSONArray;
import org.json.JSONObject;

/** Foreground-only controller. No background input service or clipboard access. */
public final class MainActivity extends Activity {
  private final Handler main = new Handler(Looper.getMainLooper());
  private final ExecutorService network = Executors.newFixedThreadPool(2);
  private final AtomicInteger generation = new AtomicInteger();
  private final AtomicLong session = new AtomicLong(), pairing = new AtomicLong();
  private final Map<String, Discovery.Peer> peers = new LinkedHashMap<>();
  private DeviceStore store;
  private Discovery discovery;
  private String identity;
  private TextView status;
  private LinearLayout saved, found, root;
  private SurfaceView surface;
  private FrameLayout viewport;
  private volatile FrameDecoder decoder;
  private volatile java.util.concurrent.CountDownLatch decoderReady =
      new java.util.concurrent.CountDownLatch(1);
  private AlertDialog fingerprint;
  private volatile boolean foreground, ready, screen, input, textInput;
  private int videoWidth, videoHeight;
  private boolean dragging, twoFingers;
  private float lastX, lastY;
  private JSONObject currentTarget;

  @Override
  public void onCreate(Bundle state) {
    super.onCreate(state);
    store = new DeviceStore(this);
    identity = store.identity();
    discovery =
        new Discovery(
            this,
            new Discovery.Listener() {
              public void changed(Map<String, Discovery.Peer> current) {
                peers.clear();
                peers.putAll(current);
                if (session.get() == 0) renderFound();
              }

              public void error(String message) {
                setStatus(message);
              }
            });
    home();
  }

  private int dp(float value) {
    return Math.round(value * getResources().getDisplayMetrics().density);
  }

  private TextView label(String text, int size) {
    TextView t = new TextView(this);
    t.setText(text);
    t.setTextSize(size);
    t.setTextColor(Color.rgb(25, 38, 62));
    t.setPadding(0, dp(7), 0, dp(7));
    return t;
  }

  private Button button(String text, Runnable action) {
    Button b = new Button(this);
    b.setText(text);
    b.setAllCaps(false);
    b.setOnClickListener(v -> action.run());
    return b;
  }

  private LinearLayout column() {
    LinearLayout v = new LinearLayout(this);
    v.setOrientation(LinearLayout.VERTICAL);
    return v;
  }

  private void home() {
    root = column();
    root.setPadding(dp(18), dp(14), dp(18), dp(10));
    root.setBackgroundColor(Color.rgb(244, 246, 250));
    root.addView(label("keyboard-it", 28));
    root.addView(label("Android’den bilgisayarını kontrol et", 15));
    status =
        label("Aynı yerel ağa bağlanın. Windows: ekran ve giriş; Mac: yalnız klavye/fare.", 14);
    root.addView(status);
    LinearLayout actions = new LinearLayout(this);
    actions.addView(
        button(
            "Cihaz ara",
            () -> {
              setStatus("Ağdaki bilgisayarlar aranıyor…");
              discovery.start();
            }),
        new LinearLayout.LayoutParams(0, -2, 1));
    actions.addView(button("IP ile ekle", this::manual), new LinearLayout.LayoutParams(0, -2, 1));
    root.addView(actions);
    ScrollView scroll = new ScrollView(this);
    LinearLayout lists = column();
    saved = column();
    found = column();
    lists.addView(label("Cihazlarım", 20));
    lists.addView(saved);
    lists.addView(label("Ağda bulunanlar", 20));
    lists.addView(found);
    scroll.addView(lists);
    root.addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
    root.addView(
        label(
            "Eşleştirmede 8 parmak izi grubunun tamamını bilgisayarla karşılaştırın. Erişimi"
                + " bilgisayarın kendi ayarlarından iptal edebilirsiniz.",
            12));
    content(root);
    renderSaved();
    renderFound();
    // Remove remote content before making the new home window capturable.
    getWindow()
        .clearFlags(
            WindowManager.LayoutParams.FLAG_SECURE
                | WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
  }

  @SuppressWarnings("deprecation")
  private void content(LinearLayout view) {
    int l = view.getPaddingLeft(),
        t = view.getPaddingTop(),
        r = view.getPaddingRight(),
        b = view.getPaddingBottom();
    view.setOnApplyWindowInsetsListener(
        (v, insets) -> {
          v.setPadding(
              l + insets.getSystemWindowInsetLeft(),
              t + insets.getSystemWindowInsetTop(),
              r + insets.getSystemWindowInsetRight(),
              b + insets.getSystemWindowInsetBottom());
          return insets;
        });
    setContentView(view);
    view.requestApplyInsets();
  }

  private void setStatus(String text) {
    if (status != null) status.setText(text);
  }

  private void renderSaved() {
    if (saved == null) return;
    saved.removeAllViews();
    try {
      JSONArray devices = store.load();
      if (devices.length() == 0) saved.addView(label("Henüz eşleştirilmiş cihaz yok.", 14));
      for (int i = 0; i < devices.length(); i++) {
        JSONObject target = devices.getJSONObject(i);
        LinearLayout row = new LinearLayout(this);
        row.addView(
            button(target.getString("name") + " — Bağlan", () -> connect(target)),
            new LinearLayout.LayoutParams(0, -2, 1));
        row.addView(
            button(
                "Unut",
                () ->
                    new AlertDialog.Builder(this)
                        .setTitle("Cihazı unut?")
                        .setMessage(
                            "Yalnız bu Android’deki kayıt silinir. Bilgisayardaki erişimi ayrıca"
                                + " iptal edebilirsiniz.")
                        .setNegativeButton("Vazgeç", null)
                        .setPositiveButton(
                            "Unut",
                            (d, w) -> {
                              try {
                                store.forget(target.getString("device_id"));
                                renderSaved();
                              } catch (Exception e) {
                                setStatus("Kayıt silinemedi.");
                              }
                            })
                        .show()));
        saved.addView(row);
      }
    } catch (Exception e) {
      setStatus("Şifreli cihaz kaydı açılamadı. Var olan kayıtlar değiştirilmedi.");
    }
  }

  private void renderFound() {
    if (found == null) return;
    found.removeAllViews();
    if (peers.isEmpty())
      found.addView(
          label(
              "Cihaz ara’yı kullanın. Misafir ağları/cihaz izolasyonu keşfi engelleyebilir.", 14));
    for (Discovery.Peer peer : peers.values())
      found.addView(button(peer.name + " (" + peer.host + ") — Eşleştir", () -> pair(peer)));
  }

  private void manual() {
    LinearLayout fields = column();
    fields.setPadding(dp(20), 0, dp(20), 0);
    EditText host = new EditText(this);
    host.setHint("Bilgisayarın IP adresi");
    host.setSingleLine(true);
    fields.addView(host);
    EditText port = new EditText(this);
    port.setHint("Eşleştirme portu");
    port.setInputType(2);
    port.setText("5600");
    fields.addView(port);
    EditText desktop = new EditText(this);
    desktop.setHint("Masaüstü portu");
    desktop.setInputType(2);
    desktop.setText("5601");
    fields.addView(desktop);
    AlertDialog dialog =
        new AlertDialog.Builder(this)
            .setTitle("IP ile eşleştir")
            .setView(fields)
            .setNegativeButton("Vazgeç", null)
            .setPositiveButton("Eşleştir", null)
            .create();
    dialog.setOnShowListener(
        d ->
            dialog
                .getButton(-1)
                .setOnClickListener(
                    v -> {
                      try {
                        Discovery.Peer peer = new Discovery.Peer();
                        peer.host = host.getText().toString().trim();
                        peer.name = peer.host;
                        peer.pairingPort = Integer.parseInt(port.getText().toString());
                        peer.desktopPort = Integer.parseInt(desktop.getText().toString());
                        dialog.dismiss();
                        pair(peer);
                      } catch (NumberFormatException e) {
                        port.setError("Geçerli port girin");
                      }
                    }));
    dialog.show();
  }

  private void pair(Discovery.Peer peer) {
    cancelAll();
    final int epoch = generation.get();
    discovery.stop();
    setStatus(peer.name + " ile güvenli eşleştirme başlatılıyor…");
    network.execute(
        () -> {
          long handle = 0;
          try {
            handle =
                Native.pairBegin(
                    peer.host,
                    peer.pairingPort,
                    peer.desktopPort,
                    android.os.Build.MODEL,
                    identity);
            if (epoch != generation.get()) {
              Native.pairCancel(handle);
              return;
            }
            pairing.set(handle);
            while (epoch == generation.get() && foreground) {
              String encoded = Native.pairPoll(handle);
              if (encoded == null) continue;
              JSONObject event = new JSONObject(encoded);
              String type = event.getString("type");
              if (type.equals("fingerprint")) {
                String code = event.getString("code");
                main.post(
                    () -> {
                      if (epoch == generation.get() && foreground) showFingerprint(code, peer.name);
                    });
              } else if (type.equals("paired")) {
                JSONObject target = event.getJSONObject("target");
                // Only the still-current foreground attempt may persist a credential.
                synchronized (generation) {
                  if (epoch != generation.get() || !foreground) return;
                  store.remember(target);
                }
                main.post(
                    () -> {
                      if (epoch == generation.get()) {
                        dismissFingerprint();
                        setStatus("Eşleştirildi. Cihazlarım’dan Bağlan’ı seçin.");
                        renderSaved();
                      }
                    });
                return;
              } else
                throw new java.io.IOException(event.optString("message", "Eşleştirme reddedildi"));
            }
          } catch (Exception error) {
            final String message = error.getMessage();
            main.post(
                () -> {
                  if (epoch == generation.get()) {
                    dismissFingerprint();
                    setStatus("Eşleştirme tamamlanamadı: " + message);
                  }
                });
          } finally {
            if (handle != 0) {
              pairing.compareAndSet(handle, 0);
              Native.pairCancel(handle);
            }
          }
        });
  }

  private void showFingerprint(String code, String name) {
    dismissFingerprint();
    TextView text = label(code.replace(" ", "  "), 16);
    text.setTypeface(Typeface.MONOSPACE);
    text.setTextIsSelectable(true);
    text.setPadding(dp(20), dp(10), dp(20), dp(10));
    ScrollView scroll = new ScrollView(this);
    scroll.addView(text);
    fingerprint =
        new AlertDialog.Builder(this)
            .setTitle(name + " — 8 grubun TAMAMINI karşılaştır")
            .setMessage(
                "Aşağıdaki parmak izi bilgisayarda gösterilenle aynıysa bilgisayarda İzin Ver’i"
                    + " seçin. Yalnız ilk grubu karşılaştırmayın.")
            .setView(scroll)
            .setNegativeButton(
                "İptal",
                (d, w) -> {
                  cancelAll();
                  setStatus("Eşleştirme iptal edildi.");
                })
            .create();
    fingerprint.setCanceledOnTouchOutside(false);
    fingerprint.setOnCancelListener(
        d -> {
          cancelAll();
          setStatus("Eşleştirme iptal edildi.");
        });
    fingerprint.show();
  }

  private void dismissFingerprint() {
    if (fingerprint != null) {
      fingerprint.dismiss();
      fingerprint = null;
    }
  }

  private JSONObject resolveTarget(JSONObject savedTarget) throws Exception {
    JSONObject target = new JSONObject(savedTarget.toString());
    for (Discovery.Peer peer : peers.values())
      if (peer.identity.equals(target.getString("device_id"))) {
        target.put("host", peer.host);
        target.put("desktop_port", peer.desktopPort);
        break;
      }
    return target; // Native pins the authenticated ID before this address is persisted.
  }

  private void connect(JSONObject savedTarget) {
    cancelAll();
    final int epoch = generation.get();
    discovery.stop();
    setStatus("Kimlik doğrulanıyor…");
    final JSONObject target;
    try {
      target = resolveTarget(savedTarget);
    } catch (Exception e) {
      setStatus("Cihaz kaydı geçersiz.");
      return;
    }
    network.execute(
        () -> {
          long handle = 0;
          try {
            handle = Native.connect(target.toString(), identity);
            if (epoch != generation.get() || !foreground) {
              Native.close(handle);
              return;
            }
            session.set(handle);
            currentTarget = target;
            synchronized (generation) {
              if (epoch != generation.get() || !foreground) return;
              store.remember(target);
            }
            while (epoch == generation.get() && foreground) {
              byte[] packet = Native.poll(handle);
              if (packet == null) continue;
              int kind = packet[0] & 255;
              if (kind == 0)
                throw new java.io.IOException(
                    new String(packet, 1, packet.length - 1, StandardCharsets.UTF_8));
              if (kind == 4) {
                screen = packet[1] != 0;
                input = packet[2] != 0;
                textInput = packet[3] != 0;
                main.post(
                    () -> {
                      if (epoch == generation.get()) controlPage(target, epoch);
                    });
              } else if (kind == 1) {
                ByteBuffer b = ByteBuffer.wrap(packet);
                b.get();
                int width = b.getInt(), height = b.getInt();
                java.util.concurrent.CountDownLatch latch =
                    new java.util.concurrent.CountDownLatch(1);
                decoderReady = latch;
                main.post(
                    () -> {
                      if (epoch == generation.get()) {
                        if (decoder != null) {
                          decoder.close();
                          decoder = null;
                        }
                        videoWidth = width;
                        videoHeight = height;
                        startDecoder(epoch);
                      }
                    });
                if (!latch.await(3, java.util.concurrent.TimeUnit.SECONDS))
                  throw new java.io.IOException("Ekran yüzeyi hazırlanamadı; yeniden bağlanın");
              } else if (kind == 2) {
                FrameDecoder d = decoder;
                if (d == null || !d.offer(java.util.Arrays.copyOfRange(packet, 2, packet.length)))
                  throw new java.io.IOException(
                      "Video çözücü hazır değil veya kuyruk dolu; yeniden bağlanın");
              } else if (kind == 3) {
                String message = new String(packet, 1, packet.length - 1, StandardCharsets.UTF_8);
                main.post(
                    () -> {
                      if (epoch == generation.get()) setStatus(message);
                    });
              }
            }
          } catch (Exception error) {
            final String message = error.getMessage();
            main.post(
                () -> {
                  if (epoch == generation.get()) {
                    cancelAll();
                    home();
                    setStatus("Bağlantı kapandı: " + message);
                  }
                });
          } finally {
            if (handle != 0) {
              session.compareAndSet(handle, 0);
              Native.close(handle);
            }
          }
        });
  }

  private void controlPage(JSONObject target, int epoch) {
    getWindow()
        .addFlags(
            WindowManager.LayoutParams.FLAG_SECURE
                | WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
    root = column();
    root.setBackgroundColor(Color.rgb(244, 246, 250));
    LinearLayout toolbar = new LinearLayout(this);
    toolbar.addView(
        button(
            "Bağlantıyı kes",
            () -> {
              cancelAll();
              home();
            }),
        new LinearLayout.LayoutParams(0, -2, 1));
    toolbar.addView(button("Klavye", this::keyboard), new LinearLayout.LayoutParams(0, -2, 1));
    toolbar.addView(button("Sağ tık", () -> click(1)), new LinearLayout.LayoutParams(0, -2, 1));
    root.addView(toolbar);
    status =
        label(
            target.optString("name")
                + (screen
                    ? " — ekran hazırlanıyor…"
                    : " — yalnız dokunmatik yüzey/klavye; Mac ekran aktarmıyor"),
            14);
    status.setPadding(dp(12), dp(4), dp(12), dp(4));
    root.addView(status);
    viewport = new FrameLayout(this);
    viewport.setBackgroundColor(Color.BLACK);
    root.addView(viewport, new LinearLayout.LayoutParams(-1, 0, 1));
    if (screen) {
      surface = new SurfaceView(this);
      viewport.addView(surface, new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER));
      surface
          .getHolder()
          .addCallback(
              new SurfaceHolder.Callback() {
                public void surfaceCreated(SurfaceHolder holder) {
                  if (epoch == generation.get()) startDecoder(epoch);
                }

                public void surfaceChanged(SurfaceHolder h, int f, int w, int he) {}

                public void surfaceDestroyed(SurfaceHolder holder) {
                  if (epoch == generation.get() && session.get() != 0) {
                    cancelAll();
                    home();
                    setStatus("Ekran kapandı; yeniden bağlanın.");
                  }
                }
              });
      touch(surface, true);
      viewport.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> sizeSurface());
    } else {
      TextView pad =
          label(
              "Dokunmatik yüzey\n\n"
                  + "Tek dokunuş: sol tık\n"
                  + "Basılı tut: sağ tık\n"
                  + "Tek parmakla sürükle: fare\n"
                  + "İki parmak: kaydır\n\n"
                  + "Bu Mac ekranını paylaşmıyor.",
              18);
      pad.setTextColor(Color.WHITE);
      pad.setGravity(Gravity.CENTER);
      viewport.addView(pad, new FrameLayout.LayoutParams(-1, -1));
      touch(pad, false);
      ready = true;
    }
    LinearLayout shortcuts = new LinearLayout(this);
    for (String title : new String[] {"Esc", "Tab", "Ctrl+C", "Ctrl+V", "Enter"})
      shortcuts.addView(
          button(title, () -> shortcut(title)), new LinearLayout.LayoutParams(0, -2, 1));
    root.addView(shortcuts);
    content(root);
  }

  private void sizeSurface() {
    if (surface == null || viewport == null || videoWidth < 1 || videoHeight < 1) return;
    int w = viewport.getWidth(), h = viewport.getHeight();
    if (w < 1 || h < 1) return;
    float scale = Math.min((float) w / videoWidth, (float) h / videoHeight);
    surface.setLayoutParams(
        new FrameLayout.LayoutParams(
            Math.max(1, Math.round(videoWidth * scale)),
            Math.max(1, Math.round(videoHeight * scale)),
            Gravity.CENTER));
  }

  private void startDecoder(int epoch) {
    if (!screen || surface == null || videoWidth < 1 || !surface.getHolder().getSurface().isValid())
      return;
    if (decoder != null) return;
    ready = false;
    sizeSurface();
    decoder =
        new FrameDecoder(
            videoWidth,
            videoHeight,
            surface.getHolder().getSurface(),
            new FrameDecoder.Listener() {
              public void ready() {
                main.post(
                    () -> {
                      if (epoch == generation.get()) {
                        ready = true;
                        setStatus(
                            currentTarget.optString("name")
                                + " — bağlı · "
                                + videoWidth
                                + "×"
                                + videoHeight);
                      }
                    });
              }

              public void failed(String message) {
                main.post(
                    () -> {
                      if (epoch == generation.get()) {
                        cancelAll();
                        home();
                        setStatus(message);
                      }
                    });
              }
            });
    decoderReady.countDown();
  }

  private boolean canInput() {
    return foreground && hasWindowFocus() && ready && input && session.get() != 0;
  }

  private void send(byte[] event) {
    if (!canInput()) return;
    try {
      Native.input(session.get(), event);
    } catch (Exception e) {
      connectionError();
    }
  }

  private void absolute(float x, float y, View view) {
    if (!canInput()) return;
    try {
      int nx = Math.round(Math.max(0, Math.min(1, x / Math.max(1, view.getWidth() - 1))) * 65535);
      int ny = Math.round(Math.max(0, Math.min(1, y / Math.max(1, view.getHeight() - 1))) * 65535);
      Native.absolute(session.get(), nx, ny);
    } catch (Exception e) {
      connectionError();
    }
  }

  private void click(int button) {
    send(RemoteInput.button(button, true));
    send(RemoteInput.button(button, false));
  }

  private void key(int usage, int modifiers) {
    send(RemoteInput.key(usage, true, modifiers));
    send(RemoteInput.key(usage, false, 0));
  }

  private void shortcut(String value) {
    switch (value) {
      case "Esc":
        key(41, 0);
        break;
      case "Tab":
        key(43, 0);
        break;
      case "Enter":
        key(40, 0);
        break;
      case "Ctrl+C":
        send(RemoteInput.key(224, true, 1));
        key(6, 1);
        send(RemoteInput.key(224, false, 0));
        break;
      case "Ctrl+V":
        send(RemoteInput.key(224, true, 1));
        key(25, 1);
        send(RemoteInput.key(224, false, 0));
        break;
    }
  }

  private void touch(View view, boolean absolute) {
    GestureDetector detector =
        new GestureDetector(
            this,
            new GestureDetector.SimpleOnGestureListener() {
              public boolean onDown(MotionEvent e) {
                return true;
              }

              public boolean onSingleTapUp(MotionEvent e) {
                if (!twoFingers) {
                  if (absolute) absolute(e.getX(), e.getY(), view);
                  click(0);
                }
                return true;
              }

              public void onLongPress(MotionEvent e) {
                if (!twoFingers) {
                  if (absolute) absolute(e.getX(), e.getY(), view);
                  click(1);
                }
              }

              public boolean onScroll(MotionEvent first, MotionEvent current, float dx, float dy) {
                if (twoFingers || first == null) return true;
                if (absolute) {
                  if (!dragging) {
                    absolute(first.getX(), first.getY(), view);
                    send(RemoteInput.button(0, true));
                    dragging = true;
                  }
                  absolute(current.getX(), current.getY(), view);
                } else send(RemoteInput.move(Math.round(-dx), Math.round(-dy)));
                return true;
              }
            });
    detector.setOnDoubleTapListener(null);
    view.setOnTouchListener(
        (v, event) -> {
          int action = event.getActionMasked();
          if (action == MotionEvent.ACTION_DOWN) {
            twoFingers = false;
            dragging = false;
            lastX = event.getX();
            lastY = event.getY();
          }
          if (event.getPointerCount() > 1) {
            if (!twoFingers) {
              twoFingers = true;
              if (dragging) {
                send(RemoteInput.button(0, false));
                dragging = false;
              }
              lastY = event.getY();
            }
            if (action == MotionEvent.ACTION_MOVE) {
              float delta = event.getY() - lastY;
              if (Math.abs(delta) > dp(12)) {
                send(RemoteInput.scroll(Math.round(delta / dp(12))));
                lastY = event.getY();
              }
            }
          } else if (!twoFingers) detector.onTouchEvent(event);
          if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
            if (dragging) send(RemoteInput.button(0, false));
            dragging = false;
            twoFingers = false;
            v.performClick();
          }
          return true;
        });
  }

  private void keyboard() {
    if (session.get() == 0 || !input) return;
    LinearLayout panel = column();
    panel.setPadding(dp(18), 0, dp(18), 0);
    EditText edit = new EditText(this);
    edit.setHint("Bilgisayara yazılacak metin");
    edit.setInputType(
        android.text.InputType.TYPE_CLASS_TEXT
            | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
    edit.setFilters(new InputFilter[] {new InputFilter.LengthFilter(1024)});
    edit.setImeOptions(EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING | EditorInfo.IME_ACTION_DONE);
    panel.addView(edit);
    panel.addView(
        label(
            textInput
                ? "Metin şifreli kanaldan doğrudan yazılır; pano kullanılmaz."
                : "Bu alıcı eski: metin girişi için bilgisayarı güncelleyin. Kısayol düğmeleri ve"
                    + " fiziksel klavye kullanılabilir.",
            13));
    new AlertDialog.Builder(this)
        .setTitle("Klavye")
        .setView(panel)
        .setNegativeButton("Kapat", null)
        .setNeutralButton("Sil", (d, w) -> key(42, 0))
        .setPositiveButton(
            "Yaz",
            (d, w) -> {
              String text = edit.getText().toString();
              if (!textInput || text.isEmpty()) return;
              try {
                Native.text(session.get(), text);
              } catch (Exception e) {
                connectionError();
              }
            })
        .show();
  }

  @Override
  public boolean dispatchKeyEvent(KeyEvent event) {
    if (canInput()
        && event.getDeviceId() >= 0
        && event.getKeyCode() != KeyEvent.KEYCODE_BACK
        && event.getKeyCode() != KeyEvent.KEYCODE_VOLUME_UP
        && event.getKeyCode() != KeyEvent.KEYCODE_VOLUME_DOWN) {
      View focus = getCurrentFocus();
      if (!(focus instanceof EditText)) {
        int usage = RemoteInput.usage(event.getKeyCode());
        if (usage != 0) {
          send(
              RemoteInput.key(
                  usage, event.getAction() != KeyEvent.ACTION_UP, RemoteInput.modifiers(event)));
          return true;
        }
      }
    }
    return super.dispatchKeyEvent(event);
  }

  private void connectionError() {
    main.post(
        () -> {
          cancelAll();
          home();
          setStatus("Giriş gönderilemedi; yeniden bağlanın.");
        });
  }

  private void cancelAll() {
    synchronized (generation) {
      generation.incrementAndGet();
    }
    ready = false;
    decoderReady.countDown();
    long s = session.getAndSet(0), p = pairing.getAndSet(0);
    if (s != 0) {
      try {
        Native.release(s);
      } catch (Exception ignored) {
      }
      Native.close(s);
    }
    if (p != 0) Native.pairCancel(p);
    if (decoder != null) {
      decoder.close();
      decoder = null;
    }
    surface = null;
    viewport = null;
    videoWidth = videoHeight = 0;
    dismissFingerprint();
  }

  @Override
  protected void onResume() {
    super.onResume();
    foreground = true;
    if (session.get() == 0 && pairing.get() == 0) home();
  }

  @Override
  protected void onPause() {
    foreground = false;
    cancelAll();
    discovery.stop();
    super.onPause();
  }

  @Override
  protected void onDestroy() {
    cancelAll();
    discovery.stop();
    network.shutdownNow();
    super.onDestroy();
  }

  @Override
  public void onWindowFocusChanged(boolean focus) {
    super.onWindowFocusChanged(focus);
    if (!focus && session.get() != 0) {
      try {
        Native.release(session.get());
      } catch (Exception ignored) {
      }
    }
  }

  @Override
  public void onBackPressed() {
    if (session.get() != 0 || pairing.get() != 0) {
      cancelAll();
      home();
    } else super.onBackPressed();
  }
}
