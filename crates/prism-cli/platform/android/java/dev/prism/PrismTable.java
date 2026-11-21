package dev.prism;

import android.os.Build;
import android.view.View;
import android.view.accessibility.AccessibilityNodeInfo;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * Tables for TalkBack (#284). The `table` op gives a table its rows (each
 * row's id, whether it is a header row, its cells' ids). The table's node
 * gets a {@link AccessibilityNodeInfo.CollectionInfo} (its row and column
 * counts: TalkBack says « table, 6 rows, 4 columns »), each cell a
 * {@link AccessibilityNodeInfo.CollectionItemInfo} (its row and column, a
 * heading in the header row) and, from API 33, its column's title, which
 * TalkBack reads when the focus moves to another column. A cell is one
 * focus stop, read from its label (its text).
 */
final class PrismTable {
    private PrismTable() {}

    static void set(PrismHost.Node table, JSONArray rows) {
        List<PrismHost.Node> headers = new ArrayList<>();
        int columns = 0;
        for (int r = 0; r < rows.length(); r++) {
            JSONObject row = rows.optJSONObject(r);
            JSONArray cells = row == null ? null : row.optJSONArray("cells");
            if (cells == null) continue;
            columns = Math.max(columns, cells.length());
            if (row.optBoolean("header") && headers.isEmpty()) {
                for (int c = 0; c < cells.length(); c++) headers.add(PrismHost.nodes.get(cells.optLong(c)));
            }
        }
        final int rowCount = rows.length(), columnCount = columns;
        table.view.setAccessibilityDelegate(new View.AccessibilityDelegate() {
            @SuppressWarnings("deprecation") // CollectionInfo.obtain: its builder is API 33
            @Override public void onInitializeAccessibilityNodeInfo(View host, AccessibilityNodeInfo info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setCollectionInfo(AccessibilityNodeInfo.CollectionInfo.obtain(
                        rowCount, columnCount, false, AccessibilityNodeInfo.CollectionInfo.SELECTION_MODE_NONE));
            }
        });
        for (int r = 0; r < rows.length(); r++) {
            JSONObject row = rows.optJSONObject(r);
            JSONArray cells = row == null ? null : row.optJSONArray("cells");
            if (cells == null) continue;
            boolean header = row.optBoolean("header");
            for (int c = 0; c < cells.length(); c++) {
                PrismHost.Node cell = PrismHost.nodes.get(cells.optLong(c));
                if (cell == null) continue;
                PrismHost.Node title = header || c >= headers.size() ? null : headers.get(c);
                describe(cell, r, c, header, title);
            }
        }
    }

    private static void describe(PrismHost.Node cell, int row, int column, boolean heading, PrismHost.Node title) {
        if (Build.VERSION.SDK_INT >= 28) cell.view.setScreenReaderFocusable(true);
        cell.view.setAccessibilityDelegate(new View.AccessibilityDelegate() {
            @SuppressWarnings("deprecation") // CollectionItemInfo.obtain: its builder is API 33
            @Override public void onInitializeAccessibilityNodeInfo(View host, AccessibilityNodeInfo info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                if (Build.VERSION.SDK_INT >= 33) {
                    AccessibilityNodeInfo.CollectionItemInfo.Builder b = new AccessibilityNodeInfo.CollectionItemInfo.Builder()
                            .setRowIndex(row).setRowSpan(1)
                            .setColumnIndex(column).setColumnSpan(1)
                            .setHeading(heading);
                    if (title != null && title.a11yLabel != null) b.setColumnTitle(title.a11yLabel);
                    info.setCollectionItemInfo(b.build());
                } else {
                    info.setCollectionItemInfo(AccessibilityNodeInfo.CollectionItemInfo.obtain(
                            row, 1, column, 1, heading));
                }
            }
        });
    }
}
