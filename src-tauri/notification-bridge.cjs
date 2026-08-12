const ZlackNotificationBridge = (function buildBridge() {
  function defaultCreateId() {
    if (globalThis.crypto?.randomUUID) {
      return globalThis.crypto.randomUUID();
    }
    defaultCreateId.sequence = (defaultCreateId.sequence || 0) + 1;
    return `zlack-${Date.now().toString(36)}-${defaultCreateId.sequence}`;
  }

  function createNotificationCoordinator(options = {}) {
    const createId = options.createId || defaultCreateId;
    const contextFreshnessMs = options.contextFreshnessMs ?? 5_000;
    const contextFutureMs = options.contextFutureMs ?? 500;
    const notifications = new Map();
    let contexts = [];
    let sequence = 0;

    function recordContext(context, capturedAt = Date.now()) {
      sequence += 1;
      contexts.push({
        teamId: context.teamId,
        channelId: context.channelId,
        capturedAt,
        sequence,
      });
      if (contexts.length > 64) contexts.shift();
    }

    function registerNotification({
      createdAt = Date.now(),
      activate,
      updateContext,
    }) {
      sequence += 1;
      const notificationId = createId();
      notifications.set(notificationId, {
        activate,
        updateContext,
        createdAt,
        sequence,
        context: undefined,
        contextConsumed: false,
        deliveredContextSequence: undefined,
        nativeShown: false,
      });
      return notificationId;
    }

    function updateContext(notificationId, notification, context) {
      if (!notification.updateContext) return Promise.resolve();
      return Promise.resolve().then(() =>
        notification.updateContext({
          notificationId,
          teamId: context?.teamId || "unknown",
          channelId: context?.channelId || "unknown",
        }),
      );
    }

    function matchPendingContexts(force = false) {
      const pendingNotifications = Array.from(notifications.entries()).sort(
        (left, right) => left[1].sequence - right[1].sequence,
      );
      const availableContexts = [...contexts].sort(
        (left, right) => left.sequence - right.sequence,
      );
      if (
        !force &&
        pendingNotifications.length > 1 &&
        availableContexts.length < pendingNotifications.length
      ) {
        return [];
      }
      const memo = new Map();

      function better(left, right) {
        if (!left) return right;
        if (!right) return left;
        if (left.matches !== right.matches) {
          return left.matches > right.matches ? left : right;
        }
        if (left.timeCost !== right.timeCost) {
          return left.timeCost < right.timeCost ? left : right;
        }
        if (left.sequenceCost !== right.sequenceCost) {
          return left.sequenceCost < right.sequenceCost ? left : right;
        }
        return left;
      }

      function solve(notificationIndex, contextIndex) {
        if (
          notificationIndex >= pendingNotifications.length ||
          contextIndex >= availableContexts.length
        ) {
          return { matches: 0, timeCost: 0, sequenceCost: 0, pairs: [] };
        }
        const key = `${notificationIndex}:${contextIndex}`;
        if (memo.has(key)) return memo.get(key);

        const skipNotification = solve(notificationIndex + 1, contextIndex);
        const skipContext = solve(notificationIndex, contextIndex + 1);
        let best = better(skipNotification, skipContext);

        const [notificationId, notification] =
          pendingNotifications[notificationIndex];
        const context = availableContexts[contextIndex];
        const age = notification.createdAt - context.capturedAt;
        if (age <= contextFreshnessMs && age >= -contextFutureMs) {
          const tail = solve(notificationIndex + 1, contextIndex + 1);
          const matched = {
            matches: tail.matches + 1,
            timeCost: tail.timeCost + Math.abs(age),
            sequenceCost:
              tail.sequenceCost +
              Math.abs(notification.sequence - context.sequence),
            pairs: [[notificationId, notification, context], ...tail.pairs],
          };
          best = better(matched, best);
        }
        memo.set(key, best);
        return best;
      }

      const plan = solve(0, 0);
      const assignments = new Map(
        plan.pairs.map(([notificationId, , context]) => [
          notificationId,
          context,
        ]),
      );
      const updates = [];
      for (const [notificationId, notification] of pendingNotifications) {
        const context = assignments.get(notificationId);
        notification.context = context;
        const contextSequence = context?.sequence ?? null;
        if (
          notification.nativeShown &&
          notification.deliveredContextSequence !== contextSequence
        ) {
          notification.deliveredContextSequence = contextSequence;
          updates.push(updateContext(notificationId, notification, context));
        }
      }
      return updates;
    }

    function consumeContext(notificationId) {
      const notification = notifications.get(notificationId);
      if (!notification || notification.contextConsumed) return null;

      matchPendingContexts();
      notification.contextConsumed = true;
      notification.deliveredContextSequence =
        notification.context?.sequence ?? null;
      if (!notification.context) return null;
      return {
        teamId: notification.context.teamId,
        channelId: notification.context.channelId,
      };
    }

    function markNativeShown(notificationId) {
      const notification = notifications.get(notificationId);
      if (!notification) return Promise.resolve();
      notification.nativeShown = true;
      const contextSequence = notification.context?.sequence ?? null;
      if (notification.deliveredContextSequence === contextSequence) {
        return Promise.resolve();
      }

      notification.deliveredContextSequence = contextSequence;
      return updateContext(notificationId, notification, notification.context);
    }

    function reconcileContexts(force = false) {
      return Promise.all(matchPendingContexts(force));
    }

    function activateNotification(payload) {
      const notification = notifications.get(payload.notificationId);
      if (!notification) return false;

      notifications.delete(payload.notificationId);
      if (notification.context) {
        contexts = contexts.filter(
          (context) => context.sequence !== notification.context.sequence,
        );
      }
      matchPendingContexts();
      return notification.activate(payload) === true;
    }

    function removeNotification(notificationId) {
      const notification = notifications.get(notificationId);
      if (!notification) return false;
      notifications.delete(notificationId);
      if (notification.context) {
        contexts = contexts.filter(
          (context) => context.sequence !== notification.context.sequence,
        );
      }
      matchPendingContexts();
      return true;
    }

    return {
      activateNotification,
      consumeContext,
      markNativeShown,
      reconcileContexts,
      recordContext,
      registerNotification,
      removeNotification,
    };
  }

  function createNotificationClass(options) {
    const coordinator = options.coordinator;
    const schedule = options.schedule || setTimeout;
    const clearSchedule = options.clearSchedule || clearTimeout;
    const showNative = options.showNative;
    const updateNativeContext = options.updateNativeContext;
    const contextDelayMs = options.contextDelayMs ?? 500;
    const retentionMs = options.retentionMs ?? 10 * 60 * 1000;
    const now = options.now || Date.now;
    const logError = options.logError || console.error;
    const EventTargetClass = options.EventTargetClass || globalThis.EventTarget;
    const EventClass = options.EventClass || globalThis.Event;

    return class ZlackNotification extends EventTargetClass {
      constructor(title, notificationOptions = {}) {
        super();
        this.title = title;
        this.options = notificationOptions;
        this.body = notificationOptions.body || "";
        this.data = notificationOptions.data;
        this.tag = notificationOptions.tag || "";
        this.onclick = null;
        this.clickHandlers = new Set();

        this.notificationId = coordinator.registerNotification({
          createdAt: now(),
          activate: () => this.dispatchNativeClick(),
          updateContext: updateNativeContext
            ? (context) => updateNativeContext(context)
            : null,
        });
        this.retentionTimer = schedule(() => {
          coordinator.removeNotification(this.notificationId);
        }, retentionMs);

        if (showNative) {
          this.contextTimer = schedule(async () => {
            const context = coordinator.consumeContext(this.notificationId);
            try {
              await showNative({
                notificationId: this.notificationId,
                title: typeof title === "string" ? title : "New Message",
                body: this.body,
                teamId: context?.teamId || "unknown",
                channelId: context?.channelId || "unknown",
              });
              await coordinator.markNativeShown(this.notificationId);
            } catch (error) {
              logError("Zlack: Failed to show native notification", error);
            }
          }, contextDelayMs);
        }
      }

      static get permission() {
        return "granted";
      }

      static requestPermission(callback) {
        if (callback) callback("granted");
        return Promise.resolve("granted");
      }

      addEventListener(type, listener, options) {
        super.addEventListener(type, listener, options);
        if (type === "click" && listener) {
          this.clickHandlers.add(listener);
        }
      }

      removeEventListener(type, listener, options) {
        super.removeEventListener(type, listener, options);
        if (type === "click") {
          this.clickHandlers.delete(listener);
        }
      }

      dispatchNativeClick() {
        clearSchedule(this.retentionTimer);
        clearSchedule(this.contextTimer);
        const hasHandler =
          typeof this.onclick === "function" || this.clickHandlers.size > 0;
        if (!hasHandler) return false;

        const event = new EventClass("click");
        if (typeof this.onclick === "function") {
          try {
            this.onclick.call(this, event);
          } catch (error) {
            logError("Zlack: Notification onclick handler failed", error);
          }
        }
        this.dispatchEvent(event);
        return true;
      }

      close() {
        clearSchedule(this.retentionTimer);
        clearSchedule(this.contextTimer);
        coordinator.removeNotification(this.notificationId);
      }
    };
  }

  function restoreNotificationPermission(NotificationClass) {
    Object.defineProperties(NotificationClass, {
      permission: {
        configurable: true,
        enumerable: true,
        get: () => "granted",
      },
      requestPermission: {
        configurable: true,
        value: (callback) => {
          if (callback) callback("granted");
          return Promise.resolve("granted");
        },
        writable: true,
      },
    });
  }

  function createNativeNotificationCommands(invoke) {
    return {
      showNative: (payload) => invoke("notify", payload),
      updateNativeContext: (payload) =>
        invoke("update_notification_context", payload),
    };
  }

  return {
    createNotificationClass,
    createNotificationCoordinator,
    createNativeNotificationCommands,
    restoreNotificationPermission,
  };
})();

if (typeof module === "object" && module.exports) {
  module.exports = ZlackNotificationBridge;
}
globalThis.__ZlackNotificationBridge = ZlackNotificationBridge;
