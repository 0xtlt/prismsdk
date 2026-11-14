package dev.prism;

import android.content.Context;
import android.view.View;
import android.view.ViewGroup;

/**
 * A container whose children sit exactly at the frames Prism's layout gave
 * them (in pixels): Prism computes layout in Rust, like on iOS and macOS.
 */
public final class PrismLayout extends ViewGroup {
    /** x, y, width, height of a child, in pixels. */
    public static final class Frame extends ViewGroup.LayoutParams {
        public int x, y;

        public Frame() {
            super(0, 0);
        }
    }

    /** Scroll content: its size is the content's, not the parent's. */
    public int fixedWidth = -1, fixedHeight = -1;

    public PrismLayout(Context context) {
        super(context);
        setClipChildren(false);
        setClipToPadding(false);
    }

    public static Frame frameOf(View v) {
        ViewGroup.LayoutParams p = v.getLayoutParams();
        if (p instanceof Frame) return (Frame) p;
        Frame f = new Frame();
        v.setLayoutParams(f);
        return f;
    }

    @Override
    protected void onMeasure(int widthSpec, int heightSpec) {
        for (int i = 0; i < getChildCount(); i++) {
            View c = getChildAt(i);
            Frame f = frameOf(c);
            c.measure(MeasureSpec.makeMeasureSpec(Math.max(0, f.width), MeasureSpec.EXACTLY),
                    MeasureSpec.makeMeasureSpec(Math.max(0, f.height), MeasureSpec.EXACTLY));
        }
        setMeasuredDimension(fixedWidth >= 0 ? fixedWidth : MeasureSpec.getSize(widthSpec),
                fixedHeight >= 0 ? fixedHeight : MeasureSpec.getSize(heightSpec));
    }

    @Override
    protected void onLayout(boolean changed, int l, int t, int r, int b) {
        for (int i = 0; i < getChildCount(); i++) {
            View c = getChildAt(i);
            Frame f = frameOf(c);
            c.layout(f.x, f.y, f.x + f.width, f.y + f.height);
        }
    }

    @Override
    protected boolean checkLayoutParams(ViewGroup.LayoutParams p) {
        return p instanceof Frame;
    }

    @Override
    protected ViewGroup.LayoutParams generateDefaultLayoutParams() {
        return new Frame();
    }
}
