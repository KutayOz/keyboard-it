package com.keyboardit.controller;

import android.content.Context;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.net.wifi.WifiManager;
import android.os.Handler;
import android.os.Looper;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.LinkedHashMap;
import java.util.Map;

/** Android NSD owns multicast; TXT IDs remain untrusted address hints. */
final class Discovery {
  static final class Peer {
    String name, host, identity;
    int pairingPort, desktopPort;
  }

  interface Listener {
    void changed(Map<String, Peer> peers);

    void error(String text);
  }

  private final NsdManager manager;
  private final WifiManager.MulticastLock multicast;
  private final Handler main = new Handler(Looper.getMainLooper());
  private final Listener listener;
  private final Map<String, Peer> peers = new LinkedHashMap<>();
  private final ArrayDeque<NsdServiceInfo> pending = new ArrayDeque<>();
  private NsdManager.DiscoveryListener discovery;
  private boolean resolving, active;
  private int generation;

  Discovery(Context context, Listener listener) {
    manager = (NsdManager) context.getSystemService(Context.NSD_SERVICE);
    WifiManager wifi =
        (WifiManager) context.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
    multicast = wifi.createMulticastLock("keyboard-it-discovery");
    multicast.setReferenceCounted(false);
    this.listener = listener;
  }

  void start() {
    stop();
    active = true;
    final int epoch = ++generation;
    peers.clear();
    pending.clear();
    resolving = false;
    multicast.acquire();
    discovery =
        new NsdManager.DiscoveryListener() {
          public void onDiscoveryStarted(String s) {}

          public void onDiscoveryStopped(String s) {}

          public void onStartDiscoveryFailed(String s, int error) {
            main.post(
                () -> {
                  if (epoch == generation) {
                    listener.error(
                        "Cihaz keşfi başlatılamadı ("
                            + error
                            + "). IP ile eşleştirmeyi kullanabilirsiniz.");
                    stop();
                  }
                });
          }

          public void onStopDiscoveryFailed(String s, int error) {}

          public void onServiceFound(NsdServiceInfo service) {
            main.post(
                () -> {
                  if (active && epoch == generation && pending.size() < 32) {
                    pending.add(service);
                    resolveNext(epoch);
                  }
                });
          }

          public void onServiceLost(NsdServiceInfo service) {
            main.post(
                () -> {
                  if (epoch == generation) {
                    peers.remove(service.getServiceName());
                    listener.changed(new LinkedHashMap<>(peers));
                  }
                });
          }
        };
    try {
      manager.discoverServices("_keyboard-it._tcp.", NsdManager.PROTOCOL_DNS_SD, discovery);
    } catch (RuntimeException error) {
      stop();
      listener.error("Ağ keşfi kullanılamıyor; IP ile eşleştirin.");
    }
  }

  private static String property(NsdServiceInfo service, String key) {
    byte[] value = service.getAttributes().get(key);
    return value == null ? "" : new String(value, StandardCharsets.UTF_8);
  }

  @SuppressWarnings("deprecation")
  private void resolveNext(int epoch) {
    if (resolving || !active || pending.isEmpty() || epoch != generation) return;
    resolving = true;
    manager.resolveService(
        pending.removeFirst(),
        new NsdManager.ResolveListener() {
          public void onResolveFailed(NsdServiceInfo info, int error) {
            main.post(
                () -> {
                  if (epoch == generation) {
                    resolving = false;
                    resolveNext(epoch);
                  }
                });
          }

          public void onServiceResolved(NsdServiceInfo info) {
            main.post(
                () -> {
                  if (epoch != generation) return;
                  try {
                    if (!"2".equals(property(info, "pairv"))) return;
                    InetAddress host = info.getHost();
                    if (android.os.Build.VERSION.SDK_INT >= 34)
                      for (InetAddress candidate : info.getHostAddresses())
                        if (candidate instanceof Inet4Address) {
                          host = candidate;
                          break;
                        }
                    if (host == null
                        || host.isAnyLocalAddress()
                        || host.isMulticastAddress()
                        || host.isLinkLocalAddress()) return;
                    Peer p = new Peer();
                    p.name = info.getServiceName().replaceAll("\\p{Cntrl}", "");
                    if (p.name.length() > 64) p.name = p.name.substring(0, 64);
                    p.host = host.getHostAddress();
                    p.identity = property(info, "device_id");
                    p.pairingPort = Integer.parseInt(property(info, "pair"));
                    p.desktopPort = Integer.parseInt(property(info, "desktop_port"));
                    if (p.pairingPort < 1
                        || p.pairingPort > 65535
                        || p.desktopPort < 1
                        || p.desktopPort > 65535
                        || peers.size() >= 32) return;
                    peers.put(info.getServiceName(), p);
                    listener.changed(new LinkedHashMap<>(peers));
                  } catch (RuntimeException ignored) {
                  } finally {
                    resolving = false;
                    resolveNext(epoch);
                  }
                });
          }
        });
  }

  void stop() {
    active = false;
    generation++;
    pending.clear();
    if (discovery != null) {
      try {
        manager.stopServiceDiscovery(discovery);
      } catch (RuntimeException ignored) {
      }
      discovery = null;
    }
    if (multicast.isHeld()) multicast.release();
  }
}
