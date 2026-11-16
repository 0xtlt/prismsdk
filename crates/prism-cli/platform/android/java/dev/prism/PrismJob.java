package dev.prism;

import android.app.job.JobInfo;
import android.app.job.JobParameters;
import android.app.job.JobScheduler;
import android.app.job.JobService;
import android.content.ComponentName;
import android.content.Context;
import android.os.PersistableBundle;

import org.json.JSONObject;

import java.util.HashMap;
import java.util.Map;

/**
 * Background tasks (`cx.register_background_task`) through JobScheduler: the
 * system runs them when it sees fit, even with the app closed (the library is
 * then loaded without the Activity and the app runs without a screen).
 */
public final class PrismJob extends JobService {
    /** Android's floor for periodic jobs. */
    static final long MIN_PERIOD_MS = JobInfo.getMinPeriodMillis();

    static final Map<String, JobParameters> running = new HashMap<>();
    static PrismJob service;

    static int jobId(String task) {
        return ("prism:" + task).hashCode();
    }

    /** `{task, every (s), network, power}`. Keeps a pending job that has not changed. */
    static void schedule(Context c, JSONObject p) {
        String task = p.optString("task");
        long every = Math.max(MIN_PERIOD_MS, Math.round(p.optDouble("every", 900) * 1000));
        boolean network = p.optBoolean("network"), power = p.optBoolean("power");
        JobScheduler js = c.getSystemService(JobScheduler.class);
        JobInfo pending = js.getPendingJob(jobId(task));
        if (pending != null && pending.getIntervalMillis() == every
                && (pending.getRequiredNetwork() != null) == network
                && pending.isRequireCharging() == power) {
            return; // Rescheduling would restart its period.
        }
        PersistableBundle extras = new PersistableBundle();
        extras.putString("prism.task", task);
        JobInfo job = new JobInfo.Builder(jobId(task), new ComponentName(c, PrismJob.class))
                .setPeriodic(every)
                .setPersisted(true) // Survives reboots (RECEIVE_BOOT_COMPLETED).
                .setRequiredNetworkType(network ? JobInfo.NETWORK_TYPE_ANY : JobInfo.NETWORK_TYPE_NONE)
                .setRequiresCharging(power)
                .setExtras(extras)
                .build();
        js.schedule(job);
    }

    @Override
    public boolean onStartJob(JobParameters params) {
        String task = params.getExtras().getString("prism.task");
        if (task == null) return false;
        service = this;
        running.put(task, params);
        PrismHost.load(this);
        PrismHost.nativeBackground(task, PrismHost.baseInfo(this).toString());
        return true; // Rust calls jobFinished when the task ends.
    }

    @Override
    public boolean onStopJob(JobParameters params) {
        String task = params.getExtras().getString("prism.task");
        if (task != null && running.remove(task) != null) PrismHost.nativeBackgroundExpired(task);
        return true; // Try again later.
    }

    static void finished(String task, boolean ok) {
        // A widget button's broadcast, not a job.
        if (PrismWidget.Action.finished(task)) return;
        JobParameters p = running.remove(task);
        if (p != null && service != null) service.jobFinished(p, !ok);
    }
}
