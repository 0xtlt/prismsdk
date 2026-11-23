package dev.prism;

import android.app.DatePickerDialog;
import android.app.Dialog;
import android.app.TimePickerDialog;
import android.content.Context;
import android.content.res.ColorStateList;
import android.content.res.Configuration;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Bundle;
import android.text.Editable;
import android.text.InputType;
import android.text.TextUtils;
import android.text.TextWatcher;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputConnectionWrapper;
import android.view.inputmethod.InputMethodManager;
import android.widget.AbsListView;
import android.widget.AdapterView;
import android.widget.BaseAdapter;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.ProgressBar;
import android.widget.SeekBar;
import android.widget.Spinner;
import android.widget.TextView;

import org.json.JSONException;
import org.json.JSONObject;

import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Calendar;
import java.util.Date;
import java.util.List;
import java.util.Locale;

/**
 * Form controls (prism_core::controls): sliders (SeekBar), pickers (Spinner),
 * date and time fields (a button opening DatePickerDialog / TimePickerDialog),
 * text areas (multi-line EditText), activity spinners (indeterminate
 * ProgressBar), text input configs (input type, IME action, submit) and the
 * status role's announcements (a polite live region).
 */
final class PrismForms {
    private PrismForms() {}

    static Context ctx() { return PrismHost.themed(); }

    static float dp(float v) { return v * PrismHost.density; }

    // --- Create ------------------------------------------------------------------------------

    /** The view of a form kind, or null for the other kinds. */
    static View create(PrismHost.Node n) {
        switch (n.kind) {
            case "slider": return new Slider(ctx(), n);
            case "picker": return picker(n);
            case "date_picker": return dateField(n);
            case "text_area": return textArea(n);
            case "checkbox": return checkbox(n);
            case "radio_group": return radioGroup(n);
            case "spinner": {
                ProgressBar p = new ProgressBar(ctx(), null, android.R.attr.progressBarStyle);
                p.setIndeterminate(true);
                return p;
            }
        }
        return null;
    }

    static void change(PrismHost.Node n, double v) {
        JSONObject ev = PrismHost.event("change", n.id);
        // NaN: an optional date picker was cleared (#89), `null` on the wire.
        try { ev.put("v", Double.isNaN(v) ? JSONObject.NULL : v); } catch (JSONException ignored) {}
        PrismHost.send(ev);
    }

    // --- Slider ------------------------------------------------------------------------------

    /** A SeekBar over integer steps: `step` steps, or 1000 for a continuous slider. */
    static final class Slider extends SeekBar {
        final PrismHost.Node n;
        double min = 0, max = 1, step = 0;
        int steps = 1000;
        /** The last value reported (dragging within a step reports once). */
        double sent = Double.NaN;

        Slider(Context c, PrismHost.Node n) {
            super(c);
            this.n = n;
            setMin(0);
            setMax(steps);
            setOnSeekBarChangeListener(new OnSeekBarChangeListener() {
                public void onProgressChanged(SeekBar s, int progress, boolean fromUser) {
                    if (fromUser && !PrismHost.updating) report(progress);
                }
                public void onStartTrackingTouch(SeekBar s) {}
                public void onStopTrackingTouch(SeekBar s) {}
            });
        }

        void configure(String config) {
            String[] t = config.trim().split("\\s+");
            min = num(t, 0, 0);
            max = Math.max(min, num(t, 1, 1));
            step = Math.max(0, num(t, 2, 0));
            steps = step > 0 && max > min ? (int) Math.max(1, Math.min(100000, Math.round((max - min) / step))) : 1000;
            setMax(steps);
            // D-pad, keyboards (and the assistive step below): SliderConfig::a11y_step.
            setKeyProgressIncrement(step > 0 ? 1 : steps / 10);
            setValue(n.value);
        }

        static double num(String[] t, int i, double d) {
            try { return i < t.length ? Double.parseDouble(t[i]) : d; } catch (NumberFormatException e) { return d; }
        }

        int progressOf(double v) {
            double f = max > min ? Math.max(0, Math.min(1, (v - min) / (max - min))) : 0;
            return (int) Math.round(f * steps);
        }

        /** SliderConfig::snap of the thumb's position. */
        double valueOf(int progress) {
            if (step > 0) return Math.max(min, Math.min(max, min + progress * step));
            return min + (max - min) * progress / steps;
        }

        void setValue(double v) {
            sent = v;
            setProgress(progressOf(v));
        }

        void report(int progress) {
            double v = valueOf(progress);
            if (v == sent) return;
            sent = v;
            change(n, v);
        }

        /** TalkBack's swipe up / down: one SliderConfig::a11y_step. */
        @Override
        public boolean performAccessibilityAction(int action, Bundle args) {
            if (isEnabled() && (action == AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
                    || action == AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD)) {
                int inc = getKeyProgressIncrement();
                int p = Math.max(0, Math.min(steps, getProgress() + (action == AccessibilityNodeInfo.ACTION_SCROLL_FORWARD ? inc : -inc)));
                if (p == getProgress()) return false;
                setProgress(p);
                report(p);
                return true;
            }
            return super.performAccessibilityAction(action, args);
        }
    }

    // --- Picker ------------------------------------------------------------------------------

    /**
     * The options, then the placeholder (shown while nothing is selected;
     * hidden in the menu, so it cannot be chosen).
     */
    static final class PickerAdapter extends BaseAdapter {
        List<String> options = new ArrayList<>();
        String placeholder = "";
        Integer color;
        float size;

        public int getCount() { return options.size() + 1; }
        public Object getItem(int i) { return i < options.size() ? options.get(i) : placeholder; }
        public long getItemId(int i) { return i; }

        public View getView(int i, View convert, ViewGroup parent) {
            TextView t = convert instanceof TextView ? (TextView) convert
                    : (TextView) LayoutInflater.from(parent.getContext()).inflate(android.R.layout.simple_spinner_item, parent, false);
            style(t, i == options.size());
            t.setText((String) getItem(i));
            return t;
        }

        public View getDropDownView(int i, View convert, ViewGroup parent) {
            if (i == options.size()) {
                View none = new View(parent.getContext());
                none.setLayoutParams(new AbsListView.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0));
                none.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
                return none;
            }
            TextView t = convert instanceof TextView ? (TextView) convert
                    : (TextView) LayoutInflater.from(parent.getContext()).inflate(android.R.layout.simple_spinner_dropdown_item, parent, false);
            if (size > 0) t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
            t.setText(options.get(i));
            return t;
        }

        void style(TextView t, boolean placeholder) {
            if (size > 0) t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
            int fg = color != null ? color : PrismHost.dark() ? 0xFFF2F2F7 : 0xFF1C1C1E;
            t.setTextColor(placeholder ? (PrismHost.dark() ? 0xFF98989F : 0xFF6C6C72) : fg);
        }
    }

    static Spinner newSpinner(PickerAdapter a) {
        Context c = ctx();
        Spinner s = new Spinner(c, null, android.R.attr.spinnerStyle, 0, Spinner.MODE_DROPDOWN, c.getTheme());
        s.setAdapter(a);
        return s;
    }

    static View picker(PrismHost.Node n) {
        PickerAdapter a = new PickerAdapter();
        Spinner s = newSpinner(a);
        n.value = -1;
        s.setSelection(a.options.size(), false);
        s.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            public void onItemSelected(AdapterView<?> parent, View v, int position, long id) {
                // Programmatic selections (the value Prism set) report nothing.
                if (position >= a.options.size() || position == (int) n.value) return;
                change(n, position);
                // The app kept its value: show it again.
                if ((int) n.value != position) selectPicker(n);
            }
            public void onNothingSelected(AdapterView<?> parent) {}
        });
        return s;
    }

    static PickerAdapter adapter(PrismHost.Node n) {
        return (PickerAdapter) ((Spinner) n.view).getAdapter();
    }

    static void selectPicker(PrismHost.Node n) {
        PickerAdapter a = adapter(n);
        int i = (int) n.value;
        ((Spinner) n.view).setSelection(i >= 0 && i < a.options.size() ? i : a.options.size(), false);
        describePicker(n);
    }

    static void describePicker(PrismHost.Node n) {
        PickerAdapter a = adapter(n);
        int i = (int) n.value;
        describe(n, i >= 0 && i < a.options.size() ? a.options.get(i) : a.placeholder);
    }

    // --- Date field --------------------------------------------------------------------------

    /** The value, formatted in the app's language; a tap opens the system's dialogs. */
    static View dateField(PrismHost.Node n) {
        TextView t = new TextView(ctx());
        t.setGravity(Gravity.CENTER);
        t.setSingleLine(true);
        t.setPadding(Math.round(dp(12)), Math.round(dp(6)), Math.round(dp(12)), Math.round(dp(6)));
        t.setClickable(true);
        t.setFocusable(true);
        t.setOnClickListener(v -> openDate(n));
        // Read as a button: "Due, 18 Nov 2026, button, double-tap to activate".
        t.setAccessibilityDelegate(new View.AccessibilityDelegate() {
            @Override public void onInitializeAccessibilityNodeInfo(View host, AccessibilityNodeInfo info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setClassName(Button.class.getName());
            }
        });
        pill(t);
        return t;
    }

    static void pill(TextView t) {
        GradientDrawable d = new GradientDrawable();
        // The platform's tertiary fill (iOS's compact date picker looks the same).
        d.setColor(PrismHost.dark() ? 0x3D767680 : 0x1F767680);
        d.setCornerRadius(dp(8));
        t.setBackground(d);
    }

    /** An optional picker (#89): it may be empty (NaN) and offers "Clear". */
    static boolean optional(String config) {
        String first = config.split("\n", 2)[0];
        for (String t : first.trim().split("\\s+")) if (t.equals("optional")) return true;
        return false;
    }

    /** DateConfig::start: what an empty picker starts from (now, to the minute, within the bounds). */
    static double start(String config) {
        long now = System.currentTimeMillis();
        double minute = Math.floor(now / 60000.0) * 60;
        if (mode(config).equals("timeofday")) {
            double local = Math.floor((now + java.util.TimeZone.getDefault().getOffset(now)) / 60000.0) * 60;
            return ((local % 86400) + 86400) % 86400;
        }
        double min = bound(config, 1), max = bound(config, 2);
        if (!Double.isNaN(min)) minute = Math.max(min, minute);
        if (!Double.isNaN(max)) minute = Math.min(max, minute);
        return minute;
    }

    /** `date`, `time` or `datetime`, and the bounds in Unix seconds (NaN: none). */
    static String mode(String config) {
        String m = config.trim().split("\\s+")[0];
        return m.equals("time") || m.equals("datetime") || m.equals("timeofday") ? m : "date";
    }

    /** A time of day is the same everywhere: seconds since midnight, shown in UTC. */
    static java.util.TimeZone zone(String mode) {
        return mode.equals("timeofday") ? java.util.TimeZone.getTimeZone("UTC") : java.util.TimeZone.getDefault();
    }

    static double bound(String config, int i) {
        String[] t = config.trim().split("\\s+");
        if (i >= t.length || t[i].equals("-")) return Double.NaN;
        try { return Double.parseDouble(t[i]); } catch (NumberFormatException e) { return Double.NaN; }
    }

    /** The app's language (the env op), else the device's. */
    static Locale locale() {
        if (!TextUtils.isEmpty(PrismHost.lang)) return Locale.forLanguageTag(PrismHost.lang);
        return PrismHost.activity.getResources().getConfiguration().getLocales().get(0);
    }

    static String format(String mode, long millis) {
        Locale l = locale();
        boolean h24 = android.text.format.DateFormat.is24HourFormat(PrismHost.activity);
        boolean time = mode.equals("time") || mode.equals("timeofday");
        String skeleton = time ? (h24 ? "Hm" : "hm")
                : mode.equals("datetime") ? (h24 ? "yMMMdHm" : "yMMMdhm") : "yMMMd";
        SimpleDateFormat f = new SimpleDateFormat(android.text.format.DateFormat.getBestDateTimePattern(l, skeleton), l);
        f.setTimeZone(zone(mode));
        return f.format(new Date(millis));
    }

    static void showDate(PrismHost.Node n) {
        TextView t = (TextView) n.view;
        if (Double.isNaN(n.value)) {
            // Empty (#89): the placeholder, dimmed like a field's hint.
            t.setText(n.placeholder);
            t.setTextColor(PrismHost.dark() ? 0xFF98989F : 0xFF6C6C72);
            describe(n, n.placeholder);
            return;
        }
        String s = format(mode(n.config), Math.round(n.value * 1000));
        t.setText(s);
        Integer tint = PrismHost.color(n.style.optJSONArray("color"));
        t.setTextColor(tint != null ? tint : PrismHost.dark() ? 0xFFF2F2F7 : 0xFF1C1C1E);
        describe(n, s);
    }

    /** "Clear" on the dialog of an optional picker that has a value (#89). */
    static void offerClear(Dialog d, PrismHost.Node n) {
        if (!optional(n.config) || Double.isNaN(n.value)) return;
        android.content.DialogInterface.OnClickListener clear = (dialog, which) -> change(n, Double.NaN);
        if (d instanceof DatePickerDialog) ((DatePickerDialog) d).setButton(Dialog.BUTTON_NEUTRAL, PrismHost.clearLabel, clear);
        else if (d instanceof TimePickerDialog) ((TimePickerDialog) d).setButton(Dialog.BUTTON_NEUTRAL, PrismHost.clearLabel, clear);
    }

    /** Dialogs in the app's light or dark mode and language (month names, right to left). */
    static Context dialogContext() {
        android.view.ContextThemeWrapper c = new android.view.ContextThemeWrapper(PrismHost.activity, PrismHost.dark()
                ? android.R.style.Theme_DeviceDefault : android.R.style.Theme_DeviceDefault_Light);
        Configuration conf = new Configuration(PrismHost.activity.getResources().getConfiguration());
        conf.setLocale(locale());
        c.applyOverrideConfiguration(conf);
        return c;
    }

    static void openDate(PrismHost.Node n) {
        String mode = mode(n.config);
        Calendar cal = Calendar.getInstance(zone(mode));
        // An empty picker (#89) opens at now; nothing changes until OK.
        double v = Double.isNaN(n.value) ? start(n.config) : n.value;
        cal.setTimeInMillis(Math.round(v * 1000));
        if (mode.equals("time") || mode.equals("timeofday")) openTime(n, cal, true);
        else {
            Context c = dialogContext();
            DatePickerDialog d = new DatePickerDialog(c, 0, (picker, y, m, day) -> {
                cal.set(y, m, day);
                if (mode.equals("datetime")) openTime(n, cal, false);
                else pickDate(n, cal);
            }, cal.get(Calendar.YEAR), cal.get(Calendar.MONTH), cal.get(Calendar.DAY_OF_MONTH));
            double min = bound(n.config, 1), max = bound(n.config, 2);
            if (!Double.isNaN(min)) d.getDatePicker().setMinDate(Math.round(min * 1000));
            if (!Double.isNaN(max)) d.getDatePicker().setMaxDate(Math.round(max * 1000));
            offerClear(d, n);
            show(d);
        }
    }

    /** `first`: the picker's first dialog, the one offering "Clear". */
    static void openTime(PrismHost.Node n, Calendar cal, boolean first) {
        TimePickerDialog d = new TimePickerDialog(dialogContext(), 0, (picker, h, m) -> {
            cal.set(Calendar.HOUR_OF_DAY, h);
            cal.set(Calendar.MINUTE, m);
            cal.set(Calendar.SECOND, 0);
            pickDate(n, cal);
        }, cal.get(Calendar.HOUR_OF_DAY), cal.get(Calendar.MINUTE),
                android.text.format.DateFormat.is24HourFormat(PrismHost.activity));
        if (first) offerClear(d, n);
        show(d);
    }

    static void show(Dialog d) {
        d.show();
        if (d.getWindow() != null) {
            d.getWindow().getDecorView().setLayoutDirection(PrismHost.rtl ? View.LAYOUT_DIRECTION_RTL : View.LAYOUT_DIRECTION_LTR);
        }
    }

    /** DateConfig::clamp, then the change in Unix seconds. */
    static void pickDate(PrismHost.Node n, Calendar cal) {
        double v = cal.getTimeInMillis() / 1000.0;
        double min = bound(n.config, 1), max = bound(n.config, 2);
        if (!Double.isNaN(min)) v = Math.max(min, v);
        if (!Double.isNaN(max)) v = Math.min(max, v);
        change(n, v);
    }

    // --- Text area and text input configs ----------------------------------------------------

    static View textArea(PrismHost.Node n) {
        EditText e = new EditText(ctx()) {
            // The caret or selection moved, typing included (#86).
            @Override protected void onSelectionChanged(int start, int end) {
                super.onSelectionChanged(start, end);
                PrismInput.selectionChanged(this);
                PrismInput.rectChanged(this);
            }
            // Copy, cut and paste go to the app when it has the hooks (#217).
            // A rich input's are plain text, its chips their labels (#115).
            @Override public boolean onTextContextMenuItem(int item) {
                return PrismInput.menuItem(this, item) || PrismRich.menuItem(this, item)
                        || super.onTextContextMenuItem(item);
            }
            // A mouse drag into the other fields of a selection group (#231);
            // a tap on a rich input's chip (#115).
            @Override public boolean dispatchTouchEvent(MotionEvent ev) {
                PrismRich.touch(this, ev);
                return PrismInput.acrossTouch(this, ev) || super.dispatchTouchEvent(ev);
            }
            // A soft keyboard's Backspace with the caret at the start deletes
            // nothing and sends no key: reported as one, so an editor can join
            // a block to the one before (#294).
            @Override public InputConnection onCreateInputConnection(EditorInfo info) {
                InputConnection ic = super.onCreateInputConnection(info);
                if (ic == null) return null;
                EditText field = this;
                return new InputConnectionWrapper(ic, true) {
                    @Override public boolean deleteSurroundingText(int before, int after) {
                        if (before > 0 && after == 0) PrismInput.backspaceAtStart(field);
                        return super.deleteSurroundingText(before, after);
                    }
                    @Override public boolean deleteSurroundingTextInCodePoints(int before, int after) {
                        if (before > 0 && after == 0) PrismInput.backspaceAtStart(field);
                        return super.deleteSurroundingTextInCodePoints(before, after);
                    }
                };
            }
        };
        PrismInput.filters(e, n, -1);
        e.setSingleLine(false);
        e.setHorizontallyScrolling(false);
        e.setMinLines(3);
        e.setGravity(Gravity.TOP | Gravity.START);
        e.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES);
        e.setVerticalScrollBarEnabled(true);
        inset(e);
        e.addTextChangedListener(new TextWatcher() {
            public void beforeTextChanged(CharSequence s, int a, int b, int c) {}
            public void onTextChanged(CharSequence s, int a, int b, int c) {}
            public void afterTextChanged(Editable s) {
                if (PrismHost.updating) return;
                JSONObject ev = PrismHost.event("input", n.id);
                try { ev.put("v", s.toString()); } catch (JSONException ignored) {}
                PrismHost.send(ev);
            }
        });
        // Capped by max_height: it scrolls its own text, not the screen around it.
        e.setOnTouchListener((v, ev) -> {
            if (v.canScrollVertically(1) || v.canScrollVertically(-1)) {
                v.getParent().requestDisallowInterceptTouchEvent(ev.getActionMasked() != MotionEvent.ACTION_UP
                        && ev.getActionMasked() != MotionEvent.ACTION_CANCEL);
            }
            return false;
        });
        return e;
    }

    /** A text area's insets (measured the same way). */
    static void inset(View v) {
        int h = Math.round(dp(12)), t = Math.round(dp(8));
        v.setPaddingRelative(h, t, h, t);
    }

    /** InputConfig tokens → input type and IME action (text inputs and areas). */
    static void inputConfig(PrismHost.Node n, String config) {
        EditText e = (EditText) n.view;
        boolean multi = n.kind.equals("text_area");
        // A text area's minimum lines (#235): `rows=N`, three by default.
        if (multi) {
            int rows = 3;
            for (String t : config.split("\n", -1)[0].split(" ")) {
                if (t.startsWith("rows=")) {
                    try { rows = Math.max(1, Integer.parseInt(t.substring(5))); } catch (NumberFormatException ignored) {}
                }
            }
            e.setMinLines(rows);
        }
        boolean secure = false, autocorrect = true;
        String keyboard = "", ret = "", cap = "sentences";
        int max = -1;
        String[] lines = config.split("\n", -1);
        for (String t : lines[0].trim().split("\\s+")) {
            if (t.startsWith("max=")) {
                try { max = Integer.parseInt(t.substring(4)); } catch (NumberFormatException ignored) {}
            } else if (t.equals("secure")) secure = !multi;
            else if (t.equals("autocorrect=off")) autocorrect = false;
            else if (t.startsWith("keyboard=")) keyboard = t.substring(9);
            else if (t.startsWith("return=")) ret = t.substring(7);
            else if (t.startsWith("capitalize=")) cap = t.substring(11);
        }
        int type;
        switch (keyboard) {
            case "number": type = InputType.TYPE_CLASS_NUMBER | (secure ? InputType.TYPE_NUMBER_VARIATION_PASSWORD : 0); break;
            case "decimal": type = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL | (secure ? InputType.TYPE_NUMBER_VARIATION_PASSWORD : 0); break;
            case "phone": type = InputType.TYPE_CLASS_PHONE; break;
            default: {
                type = InputType.TYPE_CLASS_TEXT;
                if (secure) type |= InputType.TYPE_TEXT_VARIATION_PASSWORD;
                else if (keyboard.equals("email")) type |= InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS;
                else if (keyboard.equals("url")) type |= InputType.TYPE_TEXT_VARIATION_URI;
                // Addresses and passwords are never capitalized.
                if (!secure && !keyboard.equals("email") && !keyboard.equals("url")) {
                    switch (cap) {
                        case "words": type |= InputType.TYPE_TEXT_FLAG_CAP_WORDS; break;
                        case "all": type |= InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS; break;
                        case "none": break;
                        default: type |= InputType.TYPE_TEXT_FLAG_CAP_SENTENCES;
                    }
                }
                // Secure implies no suggestions (nothing learned from passwords).
                if (secure || !autocorrect) type |= InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
                else type |= InputType.TYPE_TEXT_FLAG_AUTO_CORRECT;
                if (multi) type |= InputType.TYPE_TEXT_FLAG_MULTI_LINE;
            }
        }
        if (e.getInputType() != type) {
            // Password types switch the typeface to monospace: keep the app's.
            Typeface tf = e.getTypeface();
            int start = e.getSelectionStart(), end = e.getSelectionEnd();
            e.setInputType(type);
            e.setTypeface(tf);
            if (start >= 0 && end <= e.length()) e.setSelection(start, end);
        }
        // InputConfig::max_length.
        PrismInput.filters(e, n, max);
        if (multi) return;
        // Suggestions: the lines after the first (the system's completion list).
        if (e instanceof android.widget.AutoCompleteTextView) {
            List<String> list = new ArrayList<>();
            for (int i = 1; i < lines.length; i++) if (!lines[i].isEmpty()) list.add(lines[i]);
            ((android.widget.AutoCompleteTextView) e).setAdapter(list.isEmpty() ? null
                    : new android.widget.ArrayAdapter<>(ctx(), android.R.layout.simple_dropdown_item_1line, list));
        }
        int action;
        switch (ret) {
            case "done": action = EditorInfo.IME_ACTION_DONE; break;
            case "go": action = EditorInfo.IME_ACTION_GO; break;
            case "next": action = EditorInfo.IME_ACTION_NEXT; break;
            case "search": action = EditorInfo.IME_ACTION_SEARCH; break;
            case "send": action = EditorInfo.IME_ACTION_SEND; break;
            default: action = EditorInfo.IME_ACTION_UNSPECIFIED;
        }
        if (secure) action |= EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING;
        e.setImeOptions(action);
        // The return key (or Enter on a keyboard): UiEvent::Submit.
        e.setOnEditorActionListener((v, id, key) -> {
            if (id == EditorInfo.IME_NULL && (key == null || key.getAction() != KeyEvent.ACTION_DOWN)) return key != null;
            PrismHost.send(PrismHost.event("submit", n.id));
            int a = id == EditorInfo.IME_NULL ? EditorInfo.IME_ACTION_DONE : id;
            if (a == EditorInfo.IME_ACTION_GO || a == EditorInfo.IME_ACTION_SEARCH || a == EditorInfo.IME_ACTION_SEND) {
                InputMethodManager imm = (InputMethodManager) v.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) imm.hideSoftInputFromWindow(v.getWindowToken(), 0);
            }
            // Next moves the focus, Done hides the keyboard (the defaults).
            return false;
        });
    }

    // --- Checkbox, radio group, leading icon -------------------------------------------------

    static View checkbox(PrismHost.Node n) {
        android.widget.CheckBox c = new android.widget.CheckBox(ctx());
        c.setOnCheckedChangeListener((b, on) -> {
            if (!PrismHost.updating) change(n, on ? 1 : 0);
        });
        return c;
    }

    static View radioGroup(PrismHost.Node n) {
        android.widget.RadioGroup g = new android.widget.RadioGroup(ctx());
        g.setOnCheckedChangeListener((group, checked) -> {
            if (PrismHost.updating || checked == View.NO_ID) return;
            int index = group.indexOfChild(group.findViewById(checked));
            if (index >= 0 && index != (int) n.value) change(n, index);
        });
        return g;
    }

    /** `row` or `column`, then one RadioButton per option. */
    static void radioConfig(PrismHost.Node n, String config) {
        android.widget.RadioGroup g = (android.widget.RadioGroup) n.view;
        String[] lines = config.split("\n", -1);
        boolean updating = PrismHost.updating;
        PrismHost.updating = true;
        g.removeAllViews();
        boolean row = lines[0].equals("row");
        g.setOrientation(row ? android.widget.RadioGroup.HORIZONTAL : android.widget.RadioGroup.VERTICAL);
        for (int i = 1; i < lines.length; i++) {
            android.widget.RadioButton b = new android.widget.RadioButton(g.getContext());
            b.setId(View.generateViewId());
            b.setText(lines[i]);
            if (row) b.setPaddingRelative(b.getPaddingStart(), b.getPaddingTop(), Math.round(dp(12)), b.getPaddingBottom());
            g.addView(b);
        }
        PrismHost.updating = updating;
        selectRadio(n);
        if (n.style.length() > 0) style(n);
    }

    static void selectRadio(PrismHost.Node n) {
        android.widget.RadioGroup g = (android.widget.RadioGroup) n.view;
        int i = (int) n.value;
        boolean updating = PrismHost.updating;
        PrismHost.updating = true;
        if (i >= 0 && i < g.getChildCount()) g.check(g.getChildAt(i).getId());
        else g.clearCheck();
        PrismHost.updating = updating;
    }

    /** An icon drawn from text (the SF Symbol's glyph), before the field's text. */
    static final class GlyphDrawable extends android.graphics.drawable.Drawable {
        final android.graphics.Paint paint = new android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG);
        final String glyph;

        GlyphDrawable(String glyph, float size, int color) {
            this.glyph = glyph;
            paint.setTextSize(size);
            paint.setColor(color);
            paint.setTextAlign(android.graphics.Paint.Align.CENTER);
        }

        @Override public int getIntrinsicWidth() { return Math.round(paint.getTextSize() * 1.2f); }
        @Override public int getIntrinsicHeight() { return Math.round(paint.getTextSize() * 1.2f); }
        @Override public void draw(android.graphics.Canvas c) {
            android.graphics.Rect b = getBounds();
            android.graphics.Paint.FontMetrics m = paint.getFontMetrics();
            c.drawText(glyph, b.exactCenterX(), b.exactCenterY() - (m.ascent + m.descent) / 2, paint);
        }
        @Override public void setAlpha(int a) { paint.setAlpha(a); }
        @Override public void setColorFilter(android.graphics.ColorFilter f) { paint.setColorFilter(f); }
        @Override public int getOpacity() { return android.graphics.PixelFormat.TRANSLUCENT; }
    }

    /** `TextInput::leading_icon`: a compound drawable at the start. */
    static void leadingIcon(PrismHost.Node n) {
        EditText e = (EditText) n.view;
        String g = !TextUtils.isEmpty(n.glyph) ? n.glyph : TextUtils.isEmpty(n.icon) ? "" : "•";
        if (g.isEmpty()) {
            e.setCompoundDrawablesRelativeWithIntrinsicBounds(null, null, null, null);
            return;
        }
        int color = PrismHost.dark() ? 0xFF98989F : 0xFF6C6C72;
        e.setCompoundDrawablesRelativeWithIntrinsicBounds(new GlyphDrawable(g, e.getTextSize(), color), null, null, null);
        e.setCompoundDrawablePadding(Math.round(dp(8)));
    }

    // --- Ops ---------------------------------------------------------------------------------

    static void config(PrismHost.Node n, String config) {
        n.config = config;
        switch (n.kind) {
            case "slider": ((Slider) n.view).configure(config); break;
            case "picker": {
                PickerAdapter a = adapter(n);
                a.options = new ArrayList<>();
                if (!config.isEmpty()) for (String o : config.split("\n", -1)) a.options.add(o);
                a.notifyDataSetChanged();
                selectPicker(n);
                break;
            }
            case "date_picker": showDate(n); break;
            case "text_input": case "text_area": inputConfig(n, config); break;
            case "radio_group": radioConfig(n, config); break;
            case "overlay": PrismInput.overlayConfig(n, config); break;
        }
    }

    /** Returns whether `n` is a form kind (its value is handled). */
    static boolean value(PrismHost.Node n, double v) {
        switch (n.kind) {
            case "slider": ((Slider) n.view).setValue(v); return true;
            case "picker": selectPicker(n); return true;
            case "date_picker": showDate(n); return true;
            case "spinner": return true;
            case "checkbox": ((android.widget.CheckBox) n.view).setChecked(v != 0); return true;
            case "radio_group": selectRadio(n); return true;
        }
        return false;
    }

    static boolean placeholder(PrismHost.Node n, String text) {
        if (n.kind.equals("date_picker")) {
            n.placeholder = text;
            showDate(n);
            return true;
        }
        if (!n.kind.equals("picker")) return false;
        adapter(n).placeholder = text;
        adapter(n).notifyDataSetChanged();
        describePicker(n);
        return true;
    }

    /** The tint (`color`) and the colors that follow light/dark mode. */
    static void style(PrismHost.Node n) {
        Integer tint = PrismHost.color(n.style.optJSONArray("color"));
        ColorStateList list = tint != null ? ColorStateList.valueOf(tint) : null;
        switch (n.kind) {
            case "slider": {
                SeekBar s = (SeekBar) n.view;
                // No tint: the app's accent (the themed context's own resolves to black).
                ColorStateList c = list != null ? list : ColorStateList.valueOf(PrismHost.accent());
                s.setProgressTintList(c);
                s.setThumbTintList(c);
                break;
            }
            case "picker": {
                PickerAdapter a = adapter(n);
                a.color = tint;
                float size = (float) n.style.optDouble("font", 0);
                a.size = size;
                a.notifyDataSetChanged();
                // The arrow.
                n.view.setBackgroundTintList(list);
                break;
            }
            case "date_picker": {
                TextView t = (TextView) n.view;
                t.setGravity(Gravity.CENTER);
                if (tint == null) t.setTextColor(PrismHost.dark() ? 0xFFF2F2F7 : 0xFF1C1C1E);
                if (!n.style.has("bg") || n.style.isNull("bg")) pill(t);
                // An empty one keeps its placeholder's color (#89).
                if (Double.isNaN(n.value)) showDate(n);
                break;
            }
            case "spinner": {
                ProgressBar p = (ProgressBar) n.view;
                // The theme's color until the app sets one.
                if (p.getTag() == null) p.setTag(new Object[]{p.getIndeterminateTintList()});
                p.setIndeterminateTintList(list != null ? list : (ColorStateList) ((Object[]) p.getTag())[0]);
                break;
            }
            case "radio_group": {
                android.widget.RadioGroup g = (android.widget.RadioGroup) n.view;
                float size = (float) n.style.optDouble("font", 0);
                for (int i = 0; i < g.getChildCount(); i++) {
                    TextView t = (TextView) g.getChildAt(i);
                    if (size > 0) t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
                    if (tint != null) t.setTextColor(tint);
                }
                break;
            }
            case "text_area": {
                String a = n.style.optString("talign", "left");
                ((TextView) n.view).setGravity(Gravity.TOP
                        | (a.equals("center") ? Gravity.CENTER_HORIZONTAL : a.equals("right") ? Gravity.RIGHT : Gravity.LEFT));
                // A background from the app replaces EditText's own insets: keep the text off its border.
                inset(n.view);
                break;
            }
        }
    }

    // --- Theme changes -----------------------------------------------------------------------

    /**
     * Gives a native control (a button, a field, a check box, a radio
     * button, a switch) the looks of the theme in the app's light or dark
     * mode: its text, placeholder and link colors, its background (a
     * button's chrome, a field's underline), its check mark, a switch's thumb
     * and track. A view keeps what the theme gave it when it was made, so
     * after a theme change while the app runs it showed the old mode's
     * (black text in a dark field). They come from a control made now; the
     * app's own style is applied after this.
     */
    static void retheme(PrismHost.Node n) {
        if (n.kind.equals("radio_group")) {
            ViewGroup g = (ViewGroup) n.view;
            for (int i = 0; i < g.getChildCount(); i++) retheme(g.getChildAt(i));
        } else {
            retheme(n.view);
        }
    }

    private static void retheme(View v) {
        Context c = ctx();
        TextView fresh;
        if (v instanceof android.widget.Switch) fresh = new android.widget.Switch(c);
        else if (v instanceof android.widget.RadioButton) fresh = new android.widget.RadioButton(c);
        else if (v instanceof android.widget.CheckBox) fresh = new android.widget.CheckBox(c);
        else if (v instanceof android.widget.AutoCompleteTextView) fresh = new android.widget.AutoCompleteTextView(c);
        else if (v instanceof EditText) fresh = new EditText(c);
        else if (v instanceof Button) fresh = new Button(c);
        else return;
        TextView t = (TextView) v;
        t.setTextColor(fresh.getTextColors());
        t.setHintTextColor(fresh.getHintTextColors());
        t.setLinkTextColor(fresh.getLinkTextColors());
        t.setHighlightColor(fresh.getHighlightColor());
        // A background brings its own padding: keep the view's, which its
        // layout was measured with.
        int left = v.getPaddingLeft(), top = v.getPaddingTop(), right = v.getPaddingRight(), bottom = v.getPaddingBottom();
        v.setBackground(fresh.getBackground());
        v.setPadding(left, top, right, bottom);
        if (v instanceof android.widget.CompoundButton) {
            ((android.widget.CompoundButton) v).setButtonDrawable(((android.widget.CompoundButton) fresh).getButtonDrawable());
        }
        if (v instanceof android.widget.Switch) {
            android.widget.Switch s = (android.widget.Switch) v, f = (android.widget.Switch) fresh;
            s.setThumbDrawable(f.getThumbDrawable());
            s.setTrackDrawable(f.getTrackDrawable());
        }
    }

    // --- Accessibility -----------------------------------------------------------------------

    /** The label (content description) and the value read after it. */
    static void describe(PrismHost.Node n, String state) {
        if (!TextUtils.isEmpty(n.a11yValue)) state = n.a11yValue;
        String label = n.a11yLabel;
        if (Build.VERSION.SDK_INT >= 30) {
            if (label != null) n.view.setContentDescription(label);
            n.view.setStateDescription(TextUtils.isEmpty(state) ? null : state);
        } else if (label != null) {
            n.view.setContentDescription(TextUtils.isEmpty(state) ? label : label + ", " + state);
        }
    }

    static void a11y(PrismHost.Node n, String role, String value) {
        n.a11yValue = value;
        if ("status".equals(role)) {
            if (!TextUtils.isEmpty(value) && !value.equals(n.announced)) announce(value);
            n.announced = value;
            return;
        }
        switch (n.kind) {
            case "picker": describePicker(n); break;
            case "date_picker": showDate(n); break;
            default:
                // A slider's "40 %" instead of the platform's percentage.
                if (Build.VERSION.SDK_INT >= 30 && (value != null || n.kind.equals("slider"))) {
                    n.view.setStateDescription(value);
                }
        }
    }

    static TextView announcer;

    /**
     * Says `text` (Role::Status). announceForAccessibility is deprecated in API 36:
     * a polite live region reads its new text instead. It is one pixel in the
     * window's corner, outside Prism's views.
     */
    static void announce(String text) {
        ViewGroup outer = PrismHost.root == null ? null : (ViewGroup) PrismHost.root.getParent();
        if (outer == null) return;
        if (announcer == null || announcer.getParent() != outer) {
            if (announcer != null && announcer.getParent() != null) ((ViewGroup) announcer.getParent()).removeView(announcer);
            announcer = new TextView(PrismHost.activity);
            announcer.setTextColor(Color.TRANSPARENT);
            announcer.setTextSize(1);
            announcer.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_YES);
            announcer.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
            outer.addView(announcer, new FrameLayout.LayoutParams(1, 1));
        }
        announcer.setText(text);
    }

    // --- Measure -----------------------------------------------------------------------------

    static float[] measure(String kind, String text, float size, int weight, float maxWidth) {
        int unspecified = View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED);
        switch (kind) {
            case "slider": {
                SeekBar s = new SeekBar(ctx());
                s.measure(unspecified, unspecified);
                float w = dp(200);
                return new float[]{maxWidth >= 0 ? Math.min(w, maxWidth) : w, s.getMeasuredHeight()};
            }
            case "spinner": {
                ProgressBar p = new ProgressBar(ctx(), null, text.equals("large")
                        ? android.R.attr.progressBarStyleLarge : android.R.attr.progressBarStyle);
                p.setIndeterminate(true);
                p.measure(unspecified, unspecified);
                return new float[]{p.getMeasuredWidth(), p.getMeasuredHeight()};
            }
            case "picker": {
                PickerAdapter a = new PickerAdapter();
                a.options.add(text.isEmpty() ? "—" : text);
                a.size = size;
                Spinner s = newSpinner(a);
                s.setSelection(0, false);
                s.measure(unspecified, unspecified);
                float w = s.getMeasuredWidth();
                return new float[]{maxWidth >= 0 ? Math.min(w, maxWidth) : w, s.getMeasuredHeight()};
            }
            case "date_picker": {
                // The widest month: the value is not known here.
                String mode = mode(text);
                TextView t = (TextView) dateField(new PrismHost.Node());
                t.setLayoutParams(new ViewGroup.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT));
                if (size > 0) t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
                t.setTypeface(Typeface.create(Typeface.DEFAULT, Math.max(100, Math.min(900, weight)), false));
                Calendar cal = Calendar.getInstance();
                float w = 0, h = 0;
                for (int m = 0; m < (mode.equals("time") || mode.equals("timeofday") ? 1 : 12); m++) {
                    cal.set(2026, m, 28, 22, 58);
                    t.setText(format(mode, cal.getTimeInMillis()));
                    t.measure(unspecified, unspecified);
                    w = Math.max(w, t.getMeasuredWidth());
                    h = Math.max(h, t.getMeasuredHeight());
                }
                // An optional picker's placeholder, on the second line (#89).
                String[] lines = text.split("\n", 2);
                if (lines.length > 1 && !lines[1].isEmpty()) {
                    t.setText(lines[1]);
                    t.measure(unspecified, unspecified);
                    w = Math.max(w, t.getMeasuredWidth());
                }
                return new float[]{maxWidth >= 0 ? Math.min(w, maxWidth) : w, h};
            }
            case "checkbox": {
                android.widget.CheckBox c = new android.widget.CheckBox(ctx());
                c.setText(text);
                if (size > 0) c.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
                c.setTypeface(Typeface.create(Typeface.DEFAULT, Math.max(100, Math.min(900, weight)), false));
                int ws = maxWidth >= 0 ? View.MeasureSpec.makeMeasureSpec(Math.round(maxWidth), View.MeasureSpec.AT_MOST) : unspecified;
                c.measure(ws, unspecified);
                return new float[]{c.getMeasuredWidth(), c.getMeasuredHeight()};
            }
            case "radio_group": {
                PrismHost.Node fake = new PrismHost.Node();
                fake.kind = "radio_group";
                fake.view = radioGroup(fake);
                radioConfig(fake, text);
                android.widget.RadioGroup g = (android.widget.RadioGroup) fake.view;
                for (int i = 0; i < g.getChildCount(); i++) {
                    if (size > 0) ((TextView) g.getChildAt(i)).setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
                }
                int ws = maxWidth >= 0 ? View.MeasureSpec.makeMeasureSpec(Math.round(maxWidth), View.MeasureSpec.AT_MOST) : unspecified;
                g.measure(ws, unspecified);
                return new float[]{g.getMeasuredWidth(), g.getMeasuredHeight()};
            }
            case "text_area": {
                EditText e = (EditText) textArea(new PrismHost.Node());
                inset(e);
                // Its minimum lines come as lines of text (#235): one for a rich input.
                e.setMinLines(1);
                e.setText(text);
                e.setTextSize(TypedValue.COMPLEX_UNIT_PX, size > 0 ? size : 17 * PrismHost.density);
                e.setTypeface(Typeface.create(Typeface.DEFAULT, Math.max(100, Math.min(900, weight)), false));
                float w = maxWidth >= 0 ? maxWidth : dp(280);
                e.measure(View.MeasureSpec.makeMeasureSpec(Math.round(w), View.MeasureSpec.EXACTLY), unspecified);
                return new float[]{w, e.getMeasuredHeight()};
            }
        }
        return null;
    }
}
