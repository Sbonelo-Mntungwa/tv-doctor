package com.tvdoctor.helper;

import android.content.Context;
import android.util.Log;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;

/**
 * TV Nurse's answer to TV Doctor: plain HTTP on port 8790 of the home network.
 *
 * GET /status is open, so TV Doctor can find the nurse and check its code.
 * Everything else needs the pairing code in an X-Nurse-Code header, and the
 * actions need POST.
 */
final class Server {
    static final int PORT = 8790;
    private static final String TAG = "TVNurse";
    private static Thread thread;

    private Server() {}

    static synchronized void ensureStarted(Context context) {
        if (thread != null && thread.isAlive()) return;
        final Context app = context.getApplicationContext();
        thread = new Thread(new Runnable() {
            @Override public void run() { serve(app); }
        }, "nurse-server");
        thread.setDaemon(true);
        thread.start();
    }

    private static void serve(final Context context) {
        try (ServerSocket server = new ServerSocket()) {
            server.setReuseAddress(true);
            server.bind(new InetSocketAddress(PORT));
            while (true) {
                final Socket socket = server.accept();
                new Thread(new Runnable() {
                    @Override public void run() { handle(context, socket); }
                }, "nurse-request").start();
            }
        } catch (Exception e) {
            Log.w(TAG, "server stopped", e);
        }
    }

    private static String param(String query, String name) {
        if (query == null) return "";
        for (String pair : query.split("&")) {
            int eq = pair.indexOf('=');
            if (eq > 0 && pair.substring(0, eq).equals(name)) {
                try {
                    return URLDecoder.decode(pair.substring(eq + 1), "UTF-8");
                } catch (Exception e) {
                    return "";
                }
            }
        }
        return "";
    }

    /** A package name, and nothing that could be anything else. */
    private static boolean packageName(String text) {
        return text.matches("[A-Za-z0-9_]+(\\.[A-Za-z0-9_]+)+") && text.length() < 200;
    }

    private static void handle(Context context, Socket socket) {
        try (Socket s = socket) {
            s.setSoTimeout(10000);
            BufferedReader in = new BufferedReader(
                    new InputStreamReader(s.getInputStream(), StandardCharsets.UTF_8));
            String first = in.readLine();
            if (first == null) return;
            String[] parts = first.split(" ");
            String method = parts[0];
            String target = parts.length > 1 ? parts[1] : "/";
            String path = target;
            String query = null;
            int q = target.indexOf('?');
            if (q >= 0) {
                path = target.substring(0, q);
                query = target.substring(q + 1);
            }
            String code = null;
            String line;
            while ((line = in.readLine()) != null && !line.isEmpty()) {
                int colon = line.indexOf(':');
                if (colon > 0 && line.substring(0, colon).trim().equalsIgnoreCase("X-Nurse-Code")) {
                    code = line.substring(colon + 1).trim();
                }
            }
            boolean paired = Pairing.matches(context, code);

            String body;
            String status = "200 OK";
            byte[] image = null;
            if (path.equals("/") || path.equals("/status")) {
                body = Info.status(context, paired);
            } else if (!paired) {
                status = "401 Unauthorized";
                body = Actions.error(Pairing.lockedOut()
                        ? "too many wrong codes. Wait a minute, then use the code on the TV Nurse screen"
                        : "wrong or expired code. The current one is on the TV Nurse screen");
            } else if (path.equals("/report")) {
                body = Info.report(context);
            } else if (path.equals("/screen")) {
                // TV Nurse's own screen, for documentation. Redacted unless
                // asked otherwise; the stand-in address must look like one.
                String fake = param(query, "address");
                if (!fake.matches("[0-9]{1,3}(\\.[0-9]{1,3}){3}")) fake = "192.168.1.42";
                image = MainActivity.capture(!"0".equals(param(query, "redact")), fake);
                body = image == null ? Actions.error("open TV Nurse on the TV first") : "";
            } else if (!method.equals("POST")) {
                status = "405 Method Not Allowed";
                body = Actions.error("use POST");
            } else if (path.equals("/free")) {
                body = Actions.freeMemory(context);
            } else if (path.equals("/settings")) {
                body = Actions.openSettings(context, param(query, "page"));
            } else if (path.equals("/open") || path.equals("/launch") || path.equals("/store")) {
                String pkg = param(query, "package");
                if (!packageName(pkg)) {
                    status = "400 Bad Request";
                    body = Actions.error("not a package name");
                } else if (path.equals("/open")) {
                    body = Actions.openAppPage(context, pkg);
                } else if (path.equals("/store")) {
                    body = Actions.openStore(context, pkg);
                } else {
                    body = Actions.launch(context, pkg);
                }
            } else {
                status = "404 Not Found";
                body = Actions.error("not found");
            }

            byte[] bytes = image != null ? image : body.getBytes(StandardCharsets.UTF_8);
            String type = image != null ? "image/png" : "application/json; charset=utf-8";
            OutputStream out = s.getOutputStream();
            out.write(("HTTP/1.1 " + status + "\r\n"
                    + "Content-Type: " + type + "\r\n"
                    + "Content-Length: " + bytes.length + "\r\n"
                    + "Cache-Control: no-store\r\n"
                    + "Connection: close\r\n\r\n").getBytes(StandardCharsets.UTF_8));
            out.write(bytes);
            out.flush();
        } catch (Exception e) {
            Log.w(TAG, "request failed", e);
        }
    }
}
