package com.blyss.rvnc;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.util.AttributeSet;
import android.view.MotionEvent;
import android.view.View;

public class VncView extends View {
    private Bitmap framebuffer;
    private final Matrix matrix = new Matrix();
    private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private float scaleX = 1f, scaleY = 1f;
    private float offsetX = 0f, offsetY = 0f;
    private RfbClient client;

    public VncView(Context context) {
        super(context);
        init();
    }

    public VncView(Context context, AttributeSet attrs) {
        super(context, attrs);
        init();
    }

    private void init() {
        setBackgroundColor(Color.BLACK);
    }

    public void setClient(RfbClient client) {
        this.client = client;
    }

    public void setFramebuffer(Bitmap bmp) {
        this.framebuffer = bmp;
        recalcMatrix();
    }

    private void recalcMatrix() {
        if (framebuffer == null || getWidth() == 0) return;

        float fbW = framebuffer.getWidth();
        float fbH = framebuffer.getHeight();
        float viewW = getWidth();
        float viewH = getHeight();

        float scale = Math.min(viewW / fbW, viewH / fbH);
        scaleX = scale;
        scaleY = scale;
        offsetX = (viewW - fbW * scale) / 2f;
        offsetY = (viewH - fbH * scale) / 2f;

        matrix.reset();
        matrix.postScale(scale, scale);
        matrix.postTranslate(offsetX, offsetY);
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        recalcMatrix();
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if (framebuffer != null) {
            canvas.drawBitmap(framebuffer, matrix, paint);
        }
    }

    public void refresh() {
        postInvalidate();
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        if (client == null || framebuffer == null) return false;

        float x = (event.getX() - offsetX) / scaleX;
        float y = (event.getY() - offsetY) / scaleY;

        x = Math.max(0, Math.min(x, framebuffer.getWidth() - 1));
        y = Math.max(0, Math.min(y, framebuffer.getHeight() - 1));

        int buttonMask = 0;
        switch (event.getAction()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_MOVE:
                buttonMask = 1;
                break;
            case MotionEvent.ACTION_UP:
                buttonMask = 0;
                break;
        }

        final int fx = (int) x, fy = (int) y, fb = buttonMask;
        new Thread(() -> {
            try {
                client.sendPointerEvent(fx, fy, fb);
            } catch (Exception ignored) {}
        }).start();

        return true;
    }
}
