package com.blyss.rvnc;

import android.graphics.Bitmap;
import android.util.Log;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

public class RfbClient {
    private static final String TAG = "RfbClient";

    private Socket socket;
    private DataInputStream in;
    private DataOutputStream out;

    private int width;
    private int height;
    private int bpp;
    private String serverName;

    public interface FrameListener {
        void onFrame(Bitmap bitmap);
        void onConnected(int width, int height, String name);
        void onDisconnected(String reason);
    }

    private FrameListener listener;

    public RfbClient(FrameListener listener) {
        this.listener = listener;
    }

    public void connect(String host, int port) throws IOException {
        socket = new Socket(host, port);
        socket.setTcpNoDelay(true);
        socket.setSoTimeout(10000);
        in = new DataInputStream(socket.getInputStream());
        out = new DataOutputStream(socket.getOutputStream());

        handshake();
        listener.onConnected(width, height, serverName);
    }

    private void handshake() throws IOException {
        // 1. Read server version
        byte[] version = new byte[12];
        in.readFully(version);
        String serverVersion = new String(version).trim();
        Log.i(TAG, "Server version: " + serverVersion);

        // 2. Send client version
        out.write("RFB 003.008\n".getBytes());
        out.flush();

        // 3. Read security types
        int numTypes = in.readUnsignedByte();
        byte[] types = new byte[numTypes];
        in.readFully(types);

        // 4. Choose no auth (type 1)
        out.writeByte(1);
        out.flush();

        // 5. Read security result
        int result = in.readInt();
        if (result != 0) {
            throw new IOException("Authentication failed: " + result);
        }

        // 6. Send ClientInit (shared = true)
        out.writeByte(1);
        out.flush();

        // 7. Read ServerInit
        width = in.readUnsignedShort();
        height = in.readUnsignedShort();

        // Pixel format (16 bytes)
        bpp = in.readUnsignedByte();
        byte[] pfRest = new byte[15];
        in.readFully(pfRest);

        // Server name
        int nameLen = in.readInt();
        byte[] nameBytes = new byte[nameLen];
        in.readFully(nameBytes);
        serverName = new String(nameBytes);

        Log.i(TAG, "Connected: " + width + "x" + height + " bpp=" + bpp + " name=" + serverName);

        // Set pixel format to ARGB 32bit
        sendSetPixelFormat();

        // Set encodings
        sendSetEncodings();
    }

    private void sendSetPixelFormat() throws IOException {
        byte[] msg = new byte[20];
        msg[0] = 0; // SetPixelFormat
        // 3 bytes padding
        // Pixel format:
        msg[4] = 32;  // bpp
        msg[5] = 24;  // depth
        msg[6] = 0;   // big-endian = false
        msg[7] = 1;   // true-color = true
        // red-max = 255
        msg[8] = 0; msg[9] = (byte) 255;
        // green-max = 255
        msg[10] = 0; msg[11] = (byte) 255;
        // blue-max = 255
        msg[12] = 0; msg[13] = (byte) 255;
        // shifts: R=16 G=8 B=0
        msg[14] = 16;
        msg[15] = 8;
        msg[16] = 0;
        // 3 bytes padding

        out.write(msg);
        out.flush();
        bpp = 32;
    }

    private void sendSetEncodings() throws IOException {
        byte[] msg = new byte[8];
        msg[0] = 2; // SetEncodings
        // 1 byte padding
        msg[2] = 0; msg[3] = 1; // 1 encoding
        // Raw encoding = 0
        msg[4] = 0; msg[5] = 0; msg[6] = 0; msg[7] = 0;

        out.write(msg);
        out.flush();
    }

    public void requestFrame(boolean incremental) throws IOException {
        byte[] msg = new byte[10];
        msg[0] = 3; // FramebufferUpdateRequest
        msg[1] = (byte) (incremental ? 1 : 0);
        // x = 0, y = 0
        msg[2] = 0; msg[3] = 0;
        msg[4] = 0; msg[5] = 0;
        // width, height
        msg[6] = (byte) (width >> 8); msg[7] = (byte) width;
        msg[8] = (byte) (height >> 8); msg[9] = (byte) height;

        out.write(msg);
        out.flush();
    }

    public void readFrameUpdate(Bitmap bitmap) throws IOException {
        int msgType = in.readUnsignedByte();
        if (msgType != 0) {
            throw new IOException("Unexpected message type: " + msgType);
        }

        in.readByte(); // padding
        int numRects = in.readUnsignedShort();

        for (int i = 0; i < numRects; i++) {
            int x = in.readUnsignedShort();
            int y = in.readUnsignedShort();
            int w = in.readUnsignedShort();
            int h = in.readUnsignedShort();
            int encoding = in.readInt();

            if (encoding == 0) { // Raw
                int dataLen = w * h * (bpp / 8);
                byte[] data = new byte[dataLen];
                in.readFully(data);

                // Convert BGRX to ARGB for Android Bitmap
                int[] pixels = new int[w * h];
                ByteBuffer buf = ByteBuffer.wrap(data).order(ByteOrder.LITTLE_ENDIAN);
                for (int p = 0; p < pixels.length; p++) {
                    int val = buf.getInt();
                    int b = val & 0xFF;
                    int g = (val >> 8) & 0xFF;
                    int r = (val >> 16) & 0xFF;
                    pixels[p] = 0xFF000000 | (r << 16) | (g << 8) | b;
                }

                bitmap.setPixels(pixels, 0, w, x, y, w, h);
            } else {
                throw new IOException("Unsupported encoding: " + encoding);
            }
        }

        listener.onFrame(bitmap);
    }

    public void sendPointerEvent(int x, int y, int buttonMask) throws IOException {
        byte[] msg = new byte[6];
        msg[0] = 5; // PointerEvent
        msg[1] = (byte) buttonMask;
        msg[2] = (byte) (x >> 8); msg[3] = (byte) x;
        msg[4] = (byte) (y >> 8); msg[5] = (byte) y;

        out.write(msg);
        out.flush();
    }

    public void disconnect() {
        try {
            if (socket != null) socket.close();
        } catch (IOException ignored) {}
    }

    public int getWidth() { return width; }
    public int getHeight() { return height; }
}
