const assert = require("node:assert/strict");
const test = require("node:test");

const {
  createNotificationClass,
  createNotificationCoordinator,
  createNativeNotificationCommands,
  restoreNotificationPermission,
} = require("../src-tauri/notification-bridge.cjs");

test("restores granted permission after Tauri overwrites the shim", async () => {
  class NotificationShim {}
  let requested = false;
  Object.defineProperties(NotificationShim, {
    permission: {
      configurable: true,
      get: () => "denied",
    },
    requestPermission: {
      configurable: true,
      value: () => {
        requested = true;
        return Promise.resolve("denied");
      },
      writable: true,
    },
  });

  restoreNotificationPermission(NotificationShim);

  assert.equal(NotificationShim.permission, "granted");
  assert.equal(await NotificationShim.requestPermission(), "granted");
  assert.equal(requested, false);
});

test("defers native notification IPC readiness until invocation", async () => {
  const calls = [];
  const commands = createNativeNotificationCommands(async (command, payload) => {
    calls.push({ command, payload });
  });
  const payload = {
    notificationId: "notification-1",
    title: "Title",
    body: "Body",
  };

  await commands.showNative(payload);
  await commands.updateNativeContext(payload);

  assert.deepEqual(calls, [
    { command: "notify", payload },
    { command: "update_notification_context", payload },
  ]);
});

test("matches overlapping notification contexts one-to-one", () => {
  const ids = ["notification-a", "notification-b"];
  const coordinator = createNotificationCoordinator({
    createId: () => ids.shift(),
    contextFreshnessMs: 5_000,
    contextFutureMs: 500,
  });

  coordinator.recordContext(
    { teamId: "team-a", channelId: "channel-a" },
    1_000,
  );
  const firstId = coordinator.registerNotification({
    createdAt: 1_010,
    activate: () => false,
  });
  coordinator.recordContext(
    { teamId: "team-b", channelId: "channel-b" },
    1_020,
  );
  const secondId = coordinator.registerNotification({
    createdAt: 1_030,
    activate: () => false,
  });

  assert.deepEqual(coordinator.consumeContext(firstId), {
    teamId: "team-a",
    channelId: "channel-a",
  });
  assert.deepEqual(coordinator.consumeContext(secondId), {
    teamId: "team-b",
    channelId: "channel-b",
  });
  assert.equal(coordinator.consumeContext(firstId), null);
});

test("activates only the matching notification and consumes it", () => {
  const ids = ["notification-a", "notification-b"];
  const activations = [];
  const coordinator = createNotificationCoordinator({
    createId: () => ids.shift(),
  });

  const firstId = coordinator.registerNotification({
    createdAt: 1_000,
    activate: (payload) => {
      activations.push(["first", payload.channelId]);
      return true;
    },
  });
  const secondId = coordinator.registerNotification({
    createdAt: 1_001,
    activate: (payload) => {
      activations.push(["second", payload.channelId]);
      return true;
    },
  });

  assert.equal(
    coordinator.activateNotification({
      notificationId: secondId,
      channelId: "channel-b",
    }),
    true,
  );
  assert.deepEqual(activations, [["second", "channel-b"]]);
  assert.equal(
    coordinator.activateNotification({
      notificationId: secondId,
      channelId: "channel-b",
    }),
    false,
  );
  assert.equal(
    coordinator.activateNotification({
      notificationId: firstId,
      channelId: "channel-a",
    }),
    true,
  );
  assert.deepEqual(activations, [
    ["second", "channel-b"],
    ["first", "channel-a"],
  ]);
});

test("does not let an earlier contextless notification steal a later match", async () => {
  const ids = ["notification-a", "notification-b"];
  const coordinator = createNotificationCoordinator({
    createId: () => ids.shift(),
    contextFreshnessMs: 5_000,
    contextFutureMs: 500,
  });

  const firstId = coordinator.registerNotification({
    createdAt: 1_000,
    activate: () => false,
  });
  const secondId = coordinator.registerNotification({
    createdAt: 1_100,
    activate: () => false,
  });
  coordinator.recordContext(
    { teamId: "team-b", channelId: "channel-b" },
    1_110,
  );
  await coordinator.reconcileContexts(true);

  assert.equal(coordinator.consumeContext(firstId), null);
  assert.deepEqual(coordinator.consumeContext(secondId), {
    teamId: "team-b",
    channelId: "channel-b",
  });
});

test("preserves order when notifications arrive before grouped contexts", () => {
  const ids = ["notification-a", "notification-b"];
  const coordinator = createNotificationCoordinator({
    createId: () => ids.shift(),
    contextFreshnessMs: 5_000,
    contextFutureMs: 500,
  });

  const firstId = coordinator.registerNotification({
    createdAt: 1_000,
    activate: () => false,
  });
  const secondId = coordinator.registerNotification({
    createdAt: 1_010,
    activate: () => false,
  });
  coordinator.recordContext(
    { teamId: "team-a", channelId: "channel-a" },
    1_020,
  );
  coordinator.recordContext(
    { teamId: "team-b", channelId: "channel-b" },
    1_030,
  );

  assert.deepEqual(coordinator.consumeContext(firstId), {
    teamId: "team-a",
    channelId: "channel-a",
  });
  assert.deepEqual(coordinator.consumeContext(secondId), {
    teamId: "team-b",
    channelId: "channel-b",
  });
});

test("preserves order across separate incremental context batches", async () => {
  const ids = ["notification-a", "notification-b"];
  const coordinator = createNotificationCoordinator({
    createId: () => ids.shift(),
    contextFutureMs: 5_000,
  });

  const firstId = coordinator.registerNotification({
    createdAt: 1_000,
    activate: () => false,
  });
  const secondId = coordinator.registerNotification({
    createdAt: 1_010,
    activate: () => false,
  });
  coordinator.recordContext(
    { teamId: "team-a", channelId: "channel-a" },
    1_020,
  );
  await coordinator.reconcileContexts();
  coordinator.recordContext(
    { teamId: "team-b", channelId: "channel-b" },
    1_030,
  );
  await coordinator.reconcileContexts();

  assert.deepEqual(coordinator.consumeContext(firstId), {
    teamId: "team-a",
    channelId: "channel-a",
  });
  assert.deepEqual(coordinator.consumeContext(secondId), {
    teamId: "team-b",
    channelId: "channel-b",
  });
});

test("native activation dispatches onclick and click listeners", () => {
  const coordinator = createNotificationCoordinator({
    createId: () => "notification-1",
  });
  const scheduled = [];
  const ZlackNotification = createNotificationClass({
    coordinator,
    schedule: (callback, delay) => {
      scheduled.push({ callback, delay });
      return callback;
    },
    clearSchedule: () => {},
    showNative: async () => {},
    contextDelayMs: 500,
    retentionMs: 10_000,
  });
  const calls = [];
  const notification = new ZlackNotification("Title", { body: "Body" });
  notification.onclick = () => calls.push("onclick");
  notification.addEventListener("click", () => calls.push("listener"));

  assert.equal(
    coordinator.activateNotification({
      notificationId: notification.notificationId,
    }),
    true,
  );
  assert.deepEqual(calls, ["onclick", "listener"]);
  assert.deepEqual(
    scheduled.map(({ delay }) => delay),
    [10_000, 500],
  );
});

test("handlerless activation cancels pending notification timers", () => {
  const coordinator = createNotificationCoordinator({
    createId: () => "notification-1",
  });
  const scheduled = [];
  const cleared = [];
  const ZlackNotification = createNotificationClass({
    coordinator,
    schedule: (callback, delay) => {
      const handle = { callback, delay };
      scheduled.push(handle);
      return handle;
    },
    clearSchedule: (handle) => cleared.push(handle),
    showNative: async () => {},
  });
  const notification = new ZlackNotification("Title");

  assert.equal(
    coordinator.activateNotification({
      notificationId: notification.notificationId,
    }),
    false,
  );
  assert.deepEqual(cleared, scheduled);
});

test("updates native context when telemetry arrives after notification display", async () => {
  const coordinator = createNotificationCoordinator({
    createId: () => "notification-1",
    contextFreshnessMs: 5_000,
    contextFutureMs: 5_000,
  });
  const scheduled = [];
  const shown = [];
  const updated = [];
  const ZlackNotification = createNotificationClass({
    coordinator,
    now: () => 1_000,
    schedule: (callback, delay) => {
      const handle = { callback, delay };
      scheduled.push(handle);
      return handle;
    },
    clearSchedule: () => {},
    showNative: async (payload) => shown.push(payload),
    updateNativeContext: async (payload) => updated.push(payload),
    contextDelayMs: 500,
  });
  const notification = new ZlackNotification("Title");

  const display = scheduled.find(({ delay }) => delay === 500);
  await display.callback();
  assert.equal(shown[0].teamId, "unknown");

  coordinator.recordContext(
    { teamId: "team-late", channelId: "channel-late" },
    1_600,
  );
  await coordinator.reconcileContexts();

  assert.deepEqual(updated, [
    {
      notificationId: notification.notificationId,
      teamId: "team-late",
      channelId: "channel-late",
    },
  ]);
});

test("waits for native registration before sending a late context update", async () => {
  const coordinator = createNotificationCoordinator({
    createId: () => "notification-1",
    contextFutureMs: 5_000,
  });
  const scheduled = [];
  const updated = [];
  let finishNativeRegistration;
  const ZlackNotification = createNotificationClass({
    coordinator,
    now: () => 1_000,
    schedule: (callback, delay) => {
      const handle = { callback, delay };
      scheduled.push(handle);
      return handle;
    },
    clearSchedule: () => {},
    showNative: () =>
      new Promise((resolve) => {
        finishNativeRegistration = resolve;
      }),
    updateNativeContext: async (payload) => updated.push(payload),
  });
  new ZlackNotification("Title");

  const display = scheduled.find(({ delay }) => delay === 500);
  const registration = display.callback();
  coordinator.recordContext(
    { teamId: "team-late", channelId: "channel-late" },
    1_600,
  );
  await coordinator.reconcileContexts();
  assert.deepEqual(updated, []);

  finishNativeRegistration();
  await registration;
  assert.equal(updated.length, 1);
  assert.equal(updated[0].teamId, "team-late");
});
