package com.blyss.rvnc;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.graphics.Color;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import android.widget.TextView;

/**
 * Kiosk WebView for the phone: fullscreen, under the camera cutout, no chrome.
 * Renders a page served by the host (default http://127.0.0.1:5180/).
 * Navigation is locked to the originating host; no file access; on
 * connectivity loss a no-connection screen shows and the page reloads with
 * backoff 1->30s (no hot loop).
 */
public class WebActivity extends Activity {
    private static final String DEFAULT_URL = "http://127.0.0.1:5180/";
    private static final long BACKOFF_BASE_MS = 1000;
    private static final long BACKOFF_MAX_MS = 30_000;
    private static final String OFFLINE_TEXT = new String(new char[]{
        0x041D, 0x0415, 0x0422, ' ', 0x0421, 0x0412, 0x042F, 0x0417, 0x0418
    });

    private WebView webView;
    private TextView offlineOverlay;
    private String originalHost;
    private long backoffMs = BACKOFF_BASE_MS;
    private boolean scheduledReload = false;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable reloadRunnable = new Runnable() {
        @Override
        public void run() {
            scheduledReload = false;
            webView.reload();
        }
    };

    @SuppressLint("SetJavaScriptEnabled")
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        getWindow().addFlags(
            WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON |
            WindowManager.LayoutParams.FLAG_FULLSCREEN |
            WindowManager.LayoutParams.FLAG_LAYOUT_INSET_DECOR |
            WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN
        );
        getWindow().getDecorView().setSystemUiVisibility(
            View.SYSTEM_UI_FLAG_FULLSCREEN |
            View.SYSTEM_UI_FLAG_HIDE_NAVIGATION |
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        );
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            getWindow().getAttributes().layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
        }

        String url = getIntent().getDataString();
        if (url == null || url.isEmpty()) {
            url = DEFAULT_URL;
        }
        Uri uri = Uri.parse(url);
        originalHost = uri.getHost() != null ? uri.getHost() : "127.0.0.1";

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);

        webView = new WebView(this);
        webView.setBackgroundColor(Color.BLACK);
        webView.setFitsSystemWindows(false);
        WebSettings s = webView.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setSupportZoom(false);
        s.setBuiltInZoomControls(false);
        s.setDisplayZoomControls(false);
        // Системный font_scale=1.15 поднял бы textZoom до 115%, и холст
        // монитора (ровно 800×360 CSS) перестал бы влезать в экран.
        s.setTextZoom(100);
        s.setAllowFileAccess(false);
        s.setAllowContentAccess(false);
        s.setAllowFileAccessFromFileURLs(false);
        s.setAllowUniversalAccessFromFileURLs(false);
        webView.setOnLongClickListener(v -> true);

        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                String host = request.getUrl() != null ? request.getUrl().getHost() : null;
                return host == null || !originalHost.equalsIgnoreCase(host);
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                if (request.isForMainFrame()) {
                    showOffline();
                }
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                backoffMs = BACKOFF_BASE_MS;
                handler.removeCallbacks(reloadRunnable);
                scheduledReload = false;
                offlineOverlay.setVisibility(View.GONE);
            }
        });
        webView.setWebChromeClient(new WebChromeClient());

        offlineOverlay = new TextView(this);
        offlineOverlay.setText(OFFLINE_TEXT);
        offlineOverlay.setTextColor(Color.WHITE);
        offlineOverlay.setTextSize(24);
        offlineOverlay.setGravity(Gravity.CENTER);
        offlineOverlay.setBackgroundColor(Color.BLACK);
        offlineOverlay.setVisibility(View.GONE);

        FrameLayout.LayoutParams fill = new FrameLayout.LayoutParams(
            FrameLayout.LayoutParams.MATCH_PARENT,
            FrameLayout.LayoutParams.MATCH_PARENT
        );
        root.addView(webView, fill);
        root.addView(offlineOverlay, fill);

        setContentView(root);

        // Let content draw under the system bars and the display cutout: the
        // window already uses shortEdges, but the view hierarchy still applies
        // the left cutout inset (108px) as padding unless the decor and views
        // are told not to fit system windows. The root and the WebView both
        // consume all insets, so the WebView reaches the physical left edge
        // with no black band beside the camera.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            getWindow().setDecorFitsSystemWindows(false);
        }
        root.setFitsSystemWindows(false);
        root.setOnApplyWindowInsetsListener((v, insets) -> WindowInsets.CONSUMED);
        webView.setOnApplyWindowInsetsListener((v, insets) -> WindowInsets.CONSUMED);

        webView.loadUrl(url);
    }

    private void showOffline() {
        offlineOverlay.setVisibility(View.VISIBLE);
        handler.removeCallbacks(reloadRunnable);
        if (!scheduledReload) {
            scheduledReload = true;
            handler.postDelayed(reloadRunnable, backoffMs);
            backoffMs = Math.min(backoffMs * 2, BACKOFF_MAX_MS);
        }
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (keyCode == KeyEvent.KEYCODE_BACK && webView.canGoBack()) {
            webView.goBack();
            return true;
        }
        return super.onKeyDown(keyCode, event);
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        if (webView != null) {
            webView.loadUrl("about:blank");
            webView.destroy();
        }
        super.onDestroy();
    }
}
