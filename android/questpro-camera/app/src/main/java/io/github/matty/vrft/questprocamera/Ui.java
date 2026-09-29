package io.github.matty.vrft.questprocamera;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.accessibility.AccessibilityNodeInfo;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * The desktop app's look, for the panel: one neutral ramp, one signal colour
 * that only means "needs you", and state shown by shape. Colours are the
 * desktop's {@code vrft_gui_core::palette}.
 */
final class Ui {
    static final int PAGE = 0xff0a0a0b;
    static final int SURFACE = 0xff111113;
    static final int INSET = 0xff161619;
    static final int RAISED = 0xff26262b;
    static final int SUNKEN = 0xff0c0c0e;
    static final int LINE = 0xff212125;
    static final int LINE_SOFT = 0xff1d1d21;
    static final int LINE_STRONG = 0xff2c2c31;
    static final int LINE_FOCUS = 0xff6a6a72;
    static final int TEXT = 0xfffafafa;
    static final int TEXT_2 = 0xffb0b0b0;
    static final int TEXT_3 = 0xff858585;
    static final int TEXT_4 = 0xff5f5f66;
    static final int SIGNAL = 0xffff9447;
    static final int SIGNAL_TEXT = 0xffffb27a;
    static final int SIGNAL_BG = 0xff1a120c;
    static final int SIGNAL_LINE = 0xff5a3418;

    /** How something stands, as the desktop app's status marks show it. */
    enum Tone { GOOD, WAITING, PROBLEM, OFF }

    final Context context;
    final Typeface sans;
    final Typeface medium;
    final Typeface semibold;
    final Typeface mono;
    final Typeface monoMedium;

    Ui(Context context) {
        this.context = context;
        sans = font("Geist-Regular.ttf", Typeface.DEFAULT);
        medium = font("Geist-Medium.ttf", sans);
        semibold = font("Geist-SemiBold.ttf", Typeface.DEFAULT_BOLD);
        mono = font("GeistMono-Regular.ttf", Typeface.MONOSPACE);
        monoMedium = font("GeistMono-Medium.ttf", mono);
    }

    private Typeface font(String asset, Typeface fallback) {
        try {
            return Typeface.createFromAsset(context.getAssets(), asset);
        } catch (RuntimeException missing) {
            return fallback;
        }
    }

    int dp(float value) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value,
                context.getResources().getDisplayMetrics()));
    }

    TextView text(String value, Typeface face, float sp, int color) {
        TextView view = new TextView(context);
        view.setText(value);
        view.setTypeface(face);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, sp);
        view.setTextColor(color);
        view.setIncludeFontPadding(false);
        return view;
    }

    /** A small capital label over a group, in the mono face: {@code STREAM}. */
    TextView cap(String value) {
        TextView view = text(value.toUpperCase(java.util.Locale.ROOT), monoMedium, 11, TEXT_3);
        view.setLetterSpacing(0.06f);
        return view;
    }

    TextView body(String value, float sp, int color) {
        TextView view = text(value, sans, sp, color);
        view.setLineSpacing(0, 1.35f);
        return view;
    }

    GradientDrawable box(int fill, int stroke, float radius) {
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(fill);
        if (stroke != 0) shape.setStroke(dp(1), stroke);
        shape.setCornerRadius(dp(radius));
        return shape;
    }

    /** A card: the surface with its edge. */
    LinearLayout card() {
        LinearLayout card = new LinearLayout(context);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(box(SURFACE, LINE, 16));
        card.setClipToOutline(true);
        return card;
    }

    View rule(int color) {
        View rule = new View(context);
        rule.setBackgroundColor(color);
        rule.setLayoutParams(new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(1)));
        return rule;
    }

    /** Presses and pointer hover light up the shape, as on the desktop. */
    Drawable pressable(Drawable content, int highlight, float radius) {
        return new RippleDrawable(ColorStateList.valueOf(highlight), content,
                box(Color.WHITE, 0, radius));
    }

    /** The one white button per view. */
    TextView primaryButton(String label) {
        TextView button = button(label, PAGE);
        button.setBackground(pressable(box(TEXT, 0, 10), 0x33000000, 10));
        return button;
    }

    /** A quieter button with an edge, such as Stop. */
    TextView outlineButton(String label) {
        TextView button = button(label, TEXT);
        button.setBackground(pressable(box(0x00000000, LINE_STRONG, 10), 0x22ffffff, 10));
        return button;
    }

    private TextView button(String label, int color) {
        TextView button = text(label, medium, 15, color);
        button.setGravity(Gravity.CENTER);
        button.setMinHeight(dp(44));
        button.setMinWidth(dp(120));
        button.setPadding(dp(20), 0, dp(20), 0);
        button.setClickable(true);
        button.setFocusable(true);
        return button;
    }

    /**
     * A status mark: a lit dot while live, an open arc while working, a
     * triangle in the signal colour when something needs the player, and a
     * hollow ring when off.
     */
    static final class StatusMark extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Path path = new Path();
        private final RectF oval = new RectF();
        private final float unit;
        private Tone tone = Tone.OFF;

        StatusMark(Context context) {
            super(context);
            unit = context.getResources().getDisplayMetrics().density;
            paint.setStrokeCap(Paint.Cap.ROUND);
            paint.setStrokeJoin(Paint.Join.ROUND);
        }

        void setTone(Tone next) {
            if (tone == next) return;
            tone = next;
            invalidate();
        }

        @Override protected void onMeasure(int width, int height) {
            int size = Math.round(14 * unit);
            setMeasuredDimension(size, size);
        }

        @Override protected void onDraw(Canvas canvas) {
            float cx = getWidth() / 2f;
            float cy = getHeight() / 2f;
            switch (tone) {
                case GOOD:
                    paint.setStyle(Paint.Style.FILL);
                    paint.setColor((TEXT & 0x00ffffff) | 0x29000000);
                    canvas.drawCircle(cx, cy, 5 * unit, paint);
                    paint.setColor(TEXT);
                    canvas.drawCircle(cx, cy, 3 * unit, paint);
                    break;
                case WAITING:
                    paint.setStyle(Paint.Style.STROKE);
                    paint.setStrokeWidth(1.6f * unit);
                    paint.setColor(TEXT_2);
                    float r = 4.5f * unit;
                    oval.set(cx - r, cy - r, cx + r, cy + r);
                    canvas.drawArc(oval, -90, 270, false, paint);
                    break;
                case PROBLEM:
                    paint.setStyle(Paint.Style.STROKE);
                    paint.setStrokeWidth(1.5f * unit);
                    paint.setColor(SIGNAL);
                    path.reset();
                    path.moveTo(cx, cy - 5.5f * unit);
                    path.lineTo(cx + 6 * unit, cy + 5 * unit);
                    path.lineTo(cx - 6 * unit, cy + 5 * unit);
                    path.close();
                    canvas.drawPath(path, paint);
                    canvas.drawLine(cx, cy - 1.5f * unit, cx, cy + 1 * unit, paint);
                    paint.setStyle(Paint.Style.FILL);
                    canvas.drawCircle(cx, cy + 3 * unit, 0.8f * unit, paint);
                    break;
                case OFF:
                    paint.setStyle(Paint.Style.STROKE);
                    paint.setStrokeWidth(1 * unit);
                    paint.setColor(TEXT_4);
                    canvas.drawCircle(cx, cy, 4 * unit, paint);
                    break;
            }
        }
    }

    /** An on/off switch like the desktop's: a white track when on. */
    static final class Toggle extends View {
        interface Listener { void onChanged(boolean checked); }

        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF track = new RectF();
        private final float unit;
        private boolean checked;
        private Listener listener;

        Toggle(Context context) {
            super(context);
            unit = context.getResources().getDisplayMetrics().density;
            setClickable(true);
            setFocusable(true);
            setOnClickListener(view -> {
                setChecked(!checked);
                if (listener != null) listener.onChanged(checked);
            });
        }

        void setChecked(boolean next) {
            if (checked == next) return;
            checked = next;
            invalidate();
        }

        void setListener(Listener next) { listener = next; }

        @Override protected void onMeasure(int width, int height) {
            setMeasuredDimension(Math.round(40 * unit), Math.round(24 * unit));
        }

        @Override protected void onDraw(Canvas canvas) {
            float h = getHeight();
            track.set(0, 0, getWidth(), h);
            paint.setStyle(Paint.Style.FILL);
            paint.setColor(checked ? TEXT : LINE_STRONG);
            canvas.drawRoundRect(track, h / 2, h / 2, paint);
            float radius = h / 2 - 3 * unit;
            float x = checked ? getWidth() - h / 2 : h / 2;
            paint.setColor(checked ? PAGE : (isHovered() ? TEXT_2 : TEXT_3));
            canvas.drawCircle(x, h / 2, radius, paint);
        }

        @Override public void onHoverChanged(boolean hovered) {
            super.onHoverChanged(hovered);
            invalidate();
        }

        @Override public CharSequence getAccessibilityClassName() {
            return android.widget.Switch.class.getName();
        }

        @Override public void onInitializeAccessibilityNodeInfo(AccessibilityNodeInfo info) {
            super.onInitializeAccessibilityNodeInfo(info);
            info.setCheckable(true);
            info.setChecked(checked);
        }
    }

    /** A row of choices in a sunken track; the chosen one is raised. */
    final class Segments extends LinearLayout {
        private final TextView[] options;
        private int selected = -1;

        Segments(String[] labels, int initial, java.util.function.IntConsumer onSelected) {
            super(context);
            setOrientation(HORIZONTAL);
            setBackground(box(SUNKEN, LINE, 10));
            setPadding(dp(3), dp(3), dp(3), dp(3));
            options = new TextView[labels.length];
            for (int i = 0; i < labels.length; i++) {
                TextView option = text(labels[i], monoMedium, 13, TEXT_3);
                option.setGravity(Gravity.CENTER);
                option.setMinWidth(dp(46));
                option.setPadding(dp(10), 0, dp(10), 0);
                option.setClickable(true);
                option.setFocusable(true);
                int index = i;
                option.setOnClickListener(view -> {
                    if (index == selected) return;
                    select(index);
                    onSelected.accept(index);
                });
                addView(option, new LayoutParams(LayoutParams.WRAP_CONTENT, dp(34)));
                options[i] = option;
            }
            select(initial);
        }

        private void select(int index) {
            selected = index;
            for (int i = 0; i < options.length; i++) {
                boolean on = i == index;
                options[i].setSelected(on);
                options[i].setTextColor(on ? TEXT : TEXT_3);
                options[i].setBackground(pressable(
                        on ? box(RAISED, LINE_STRONG, 7) : box(0x00000000, 0, 7),
                        0x1fffffff, 7));
            }
        }
    }
}
