package com.blyss.rvnc;

import android.app.Activity;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.os.Bundle;
import android.util.Log;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;

import java.io.IOException;
import java.io.InputStream;
import java.net.Socket;
import java.nio.ByteBuffer;

public class MainActivity extends Activity {
    private static final String TAG = "rVNC";
    private static final String HOST = "127.0.0.1";
    private static final int PORT = 8800;

    private SurfaceView surfaceView;
    private TextView statusText;
    private volatile boolean running = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        getWindow().addFlags(
            WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON |
            WindowManager.LayoutParams.FLAG_FULLSCREEN
        );
        getWindow().getDecorView().setSystemUiVisibility(
            View.SYSTEM_UI_FLAG_FULLSCREEN |
            View.SYSTEM_UI_FLAG_HIDE_NAVIGATION |
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        );

        setContentView(R.layout.activity_main);
        surfaceView = findViewById(R.id.surface_view);
        statusText = findViewById(R.id.status_text);

        surfaceView.getHolder().addCallback(new SurfaceHolder.Callback() {
            @Override
            public void surfaceCreated(SurfaceHolder holder) {
                running = true;
                new Thread(() -> connectionLoop(holder.getSurface())).start();
            }
            @Override
            public void surfaceChanged(SurfaceHolder h, int f, int w, int ht) {}
            @Override
            public void surfaceDestroyed(SurfaceHolder holder) { running = false; }
        });
    }

    private void connectionLoop(Surface surface) {
        while (running) {
            try {
                setStatus("Connecting...");
                decode(surface);
            } catch (Exception e) {
                Log.e(TAG, "Decode error: " + e.getMessage());
                setStatus("Reconnecting...");
                try { Thread.sleep(1000); } catch (InterruptedException ignored) {}
            }
        }
    }

    private void decode(Surface surface) throws IOException {
        Socket socket = new Socket(HOST, PORT);
        socket.setTcpNoDelay(true);
        socket.setReceiveBufferSize(512 * 1024);
        InputStream in = socket.getInputStream();

        MediaCodec codec = null;
        boolean started = false;

        // Buffer for accumulating stream data
        byte[] buf = new byte[2 * 1024 * 1024];
        int bufPos = 0;

        // Config data (SPS + PPS) to prepend to first keyframe
        byte[] configData = null;

        try {
            while (running) {
                int read = in.read(buf, bufPos, buf.length - bufPos);
                if (read < 0) throw new IOException("EOF");
                bufPos += read;

                // Process all complete NAL units in buffer
                int consumed = 0;
                while (true) {
                    // Find first NAL start code
                    int nalStart = findStartCode(buf, consumed, bufPos);
                    if (nalStart < 0) break;

                    // Find next NAL start code (end of current NAL)
                    int nalEnd = findStartCode(buf, nalStart + 4, bufPos);
                    if (nalEnd < 0) break; // NAL not complete yet

                    int nalType = buf[nalStart + 4] & 0x1F;
                    int nalLen = nalEnd - nalStart;

                    if (nalType == 7 || nalType == 8) {
                        // SPS or PPS — accumulate config data
                        if (nalType == 7) {
                            // New SPS — reset config
                            configData = new byte[0];
                        }
                        if (configData != null) {
                            byte[] newConfig = new byte[configData.length + nalLen];
                            System.arraycopy(configData, 0, newConfig, 0, configData.length);
                            System.arraycopy(buf, nalStart, newConfig, configData.length, nalLen);
                            configData = newConfig;
                        }
                    } else {
                        // Media NAL — initialize codec if needed
                        if (!started && configData != null && configData.length > 0) {
                            codec = createDecoder(surface, configData);
                            started = true;
                            setStatus(null);
                        }

                        if (started && codec != null) {
                            // Feed one NAL unit per input buffer (scrcpy pattern)
                            int inputIdx = codec.dequeueInputBuffer(5000);
                            if (inputIdx >= 0) {
                                ByteBuffer inputBuf = codec.getInputBuffer(inputIdx);
                                inputBuf.clear();

                                // For keyframes, prepend config if available
                                boolean isKeyframe = (nalType == 5);
                                if (isKeyframe && configData != null) {
                                    inputBuf.put(configData);
                                }

                                inputBuf.put(buf, nalStart, nalLen);
                                int flags = isKeyframe ? MediaCodec.BUFFER_FLAG_KEY_FRAME : 0;
                                codec.queueInputBuffer(inputIdx, 0, inputBuf.position(),
                                    System.nanoTime() / 1000, flags);
                            }

                            // Drain all available output — render immediately
                            drainOutput(codec);
                        }
                    }

                    consumed = nalEnd;
                }

                // Compact buffer
                if (consumed > 0) {
                    int remaining = bufPos - consumed;
                    System.arraycopy(buf, consumed, buf, 0, remaining);
                    bufPos = remaining;
                }
            }
        } finally {
            if (codec != null) {
                codec.stop();
                codec.release();
            }
            socket.close();
        }
    }

    private MediaCodec createDecoder(Surface surface, byte[] configData) throws IOException {
        // Parse SPS and PPS from config data
        byte[] sps = null;
        byte[] pps = null;

        int i = 0;
        while (i < configData.length - 4) {
            int start = findStartCode(configData, i, configData.length);
            if (start < 0) break;
            int end = findStartCode(configData, start + 4, configData.length);
            if (end < 0) end = configData.length;

            int type = configData[start + 4] & 0x1F;
            int len = end - start;
            byte[] nal = new byte[len];
            System.arraycopy(configData, start, nal, 0, len);

            if (type == 7) sps = nal;
            else if (type == 8) pps = nal;

            i = end;
        }

        MediaFormat format = MediaFormat.createVideoFormat("video/avc", 1920, 1080);
        if (sps != null) format.setByteBuffer("csd-0", ByteBuffer.wrap(sps));
        if (pps != null) format.setByteBuffer("csd-1", ByteBuffer.wrap(pps));
        format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1);
        format.setInteger("priority", 0); // realtime priority

        MediaCodec codec = MediaCodec.createDecoderByType("video/avc");
        codec.configure(format, surface, null, 0);
        codec.start();

        Log.i(TAG, "Decoder started, low-latency mode");
        return codec;
    }

    private void drainOutput(MediaCodec codec) {
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
        int outIdx;
        while ((outIdx = codec.dequeueOutputBuffer(info, 0)) >= 0) {
            // Render immediately
            codec.releaseOutputBuffer(outIdx, true);
        }
    }

    /**
     * Find H.264 Annex B start code (0x00000001) in buffer.
     * Returns offset of start code, or -1.
     */
    private static int findStartCode(byte[] buf, int from, int to) {
        for (int i = from; i < to - 3; i++) {
            if (buf[i] == 0 && buf[i+1] == 0 && buf[i+2] == 0 && buf[i+3] == 1) {
                return i;
            }
            // Also check 3-byte start code (0x000001)
            if (buf[i] == 0 && buf[i+1] == 0 && buf[i+2] == 1) {
                return i;
            }
        }
        return -1;
    }

    private void setStatus(String text) {
        runOnUiThread(() -> {
            if (text == null) {
                statusText.setVisibility(View.GONE);
            } else {
                statusText.setText(text);
                statusText.setVisibility(View.VISIBLE);
            }
        });
    }

    @Override
    protected void onDestroy() {
        running = false;
        super.onDestroy();
    }
}
