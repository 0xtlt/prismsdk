package dev.prism;

/**
 * When a widget with a refresh hook (`Widget::refresh`, #331) runs it next,
 * from its snapshot's reload policy (`"reload"`): WidgetKit's
 * `.after(date)` / `.atEnd` / `.never` on Android, where an alarm runs the
 * hook ({@link PrismWidget.Refresh}). Plain Java, so the JVM tests it.
 */
final class PrismRefreshPolicy {
    /** The shortest time between two refreshes of a widget (ms): a tighter policy would loop. */
    static final long MIN_REFRESH = 60_000;

    private PrismRefreshPolicy() {}

    /**
     * When the hook runs next, from `now` (ms), or 0 for never.
     *
     * @param refresh   the widget has a hook (`"refresh": true`)
     * @param reload    its policy: a delay in seconds (a Number), "end", "never", or null
     * @param lastAhead the instant of its last drawing ahead (s), NaN without one
     */
    static long refreshAt(boolean refresh, Object reload, double lastAhead, long now) {
        if (!refresh) return 0;
        if (reload instanceof Number) {
            double ms = ((Number) reload).doubleValue() * 1000;
            return now + Math.max(MIN_REFRESH, Double.isNaN(ms) ? 0 : Math.round(ms));
        }
        if ("end".equals(reload)) {
            // When the drawings ahead run out; never without any.
            long end = Double.isNaN(lastAhead) ? 0 : Math.round(lastAhead * 1000);
            return end > now ? Math.max(end, now + MIN_REFRESH) : 0;
        }
        return 0;
    }
}
