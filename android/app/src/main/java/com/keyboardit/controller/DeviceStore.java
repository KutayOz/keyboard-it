package com.keyboardit.controller;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import org.json.JSONArray;
import org.json.JSONObject;

/** Pairing credentials never enter backups, logs, intents, or plaintext prefs. */
final class DeviceStore {
  private static final String ALIAS = "keyboard-it.controller.devices.v1";
  private final SharedPreferences preferences;

  DeviceStore(Context context) {
    preferences = context.getSharedPreferences("devices", Context.MODE_PRIVATE);
  }

  String identity() {
    String id = preferences.getString("identity", null);
    if (id == null) {
      id = Native.identity();
      if (!preferences.edit().putString("identity", id).commit())
        throw new IllegalStateException("Kimlik kaydedilemedi");
    }
    return id;
  }

  private SecretKey key() throws Exception {
    KeyStore store = KeyStore.getInstance("AndroidKeyStore");
    store.load(null);
    if (!store.containsAlias(ALIAS)) {
      // Missing key with existing ciphertext is corruption, not a new empty registry.
      if (preferences.contains("ciphertext"))
        throw new IllegalStateException("Cihaz anahtarı kaybolmuş; kayıtları açıkça sıfırlayın");
      KeyGenerator generator =
          KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
      generator.init(
          new KeyGenParameterSpec.Builder(
                  ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
              .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
              .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
              .setKeySize(256)
              .build());
      generator.generateKey();
    }
    return (SecretKey) store.getKey(ALIAS, null);
  }

  synchronized JSONArray load() throws Exception {
    String ciphertext = preferences.getString("ciphertext", null);
    if (ciphertext == null) return new JSONArray();
    String iv = preferences.getString("iv", null);
    if (iv == null) throw new IllegalStateException("Şifreli cihaz kaydı eksik");
    Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
    cipher.init(
        Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)));
    cipher.updateAAD(ALIAS.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    JSONArray result =
        new JSONArray(
            new String(
                cipher.doFinal(Base64.decode(ciphertext, Base64.NO_WRAP)),
                java.nio.charset.StandardCharsets.UTF_8));
    if (result.length() > 32) throw new IllegalStateException("Cihaz sınırı aşıldı");
    return result;
  }

  private void save(JSONArray devices) throws Exception {
    Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
    cipher.init(Cipher.ENCRYPT_MODE, key());
    cipher.updateAAD(ALIAS.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    byte[] encrypted =
        cipher.doFinal(devices.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8));
    if (!preferences
        .edit()
        .putString("iv", Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP))
        .putString("ciphertext", Base64.encodeToString(encrypted, Base64.NO_WRAP))
        .commit()) throw new IllegalStateException("Kayıt yazılamadı");
  }

  synchronized void remember(JSONObject target) throws Exception {
    JSONArray existing = load(), result = new JSONArray();
    for (int i = 0; i < existing.length(); i++) {
      JSONObject d = existing.getJSONObject(i);
      if (!d.getString("device_id").equals(target.getString("device_id"))) result.put(d);
    }
    if (result.length() >= 32) throw new IllegalStateException("Önce bir cihazı unutun");
    result.put(target);
    save(result);
  }

  synchronized void forget(String id) throws Exception {
    JSONArray existing = load(), result = new JSONArray();
    for (int i = 0; i < existing.length(); i++)
      if (!existing.getJSONObject(i).getString("device_id").equals(id))
        result.put(existing.getJSONObject(i));
    save(result);
  }
}
