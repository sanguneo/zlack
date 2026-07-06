
// Preload script to bridge Slack notifications to Tauri

// 1. MOCK Service Workers
if (window.navigator) {
    const dummyServiceWorker = {
        controller: null,
        ready: new Promise(() => {}), // Never resolves
        getRegistration: () => Promise.resolve(undefined),
        register: () => Promise.reject(new Error("ServiceWorkers disabled in Zlack")),
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
    };

    Object.defineProperty(window.navigator, 'serviceWorker', {
        get: function() {
            return dummyServiceWorker;
        },
        configurable: true
    });
}

// 2. Mock Permission API
const originalQuery = navigator.permissions.query;
navigator.permissions.query = (parameters) => {
    if (parameters.name === 'notifications') {
        return Promise.resolve({
            state: 'granted',
            addEventListener: () => {},
            removeEventListener: () => {},
            onchange: null
        });
    }
    return originalQuery.call(navigator.permissions, parameters);
};


// 2.5 Intercept Network Requests (Telemetry) for Notification Context
let lastEventContext = { teamId: 'unknown', channelId: 'unknown', ts: 0 };
// Correlation rules for attaching channel context to a notification:
//   - every telemetry capture stamps lastEventContext.ts (Date.now()).
//   - a notification only adopts the context if it was captured within
//     CONTEXT_FRESHNESS_MS before (or NOTIFY_DELAY_MS after) the notification.
//   This prevents a channel-less notification from inheriting a previous
//   notification's team/channel (the old fixed-snapshot bug).
const NOTIFY_DELAY_MS = 500;       // grace period for a slightly-delayed telemetry beacon
const CONTEXT_FRESHNESS_MS = 5000; // max age of context still considered "this notification's"


function processTelemetryBody(bodyStr) {
    try {
        if (!bodyStr || !bodyStr.includes('notification:sent')) return;
        
        const data = JSON.parse(bodyStr);
        if (!data.spans) return;

        data.spans.forEach(span => {
            if (span.name === 'notification:sent' && span.tags) {
                // Extracted tags
                let tid = null;
                let cid = null;
                
                span.tags.forEach(tag => {
                    if (tag.key === 'encoded_team_id') tid = tag.v_str;
                    if (tag.key === 'encoded_channel_id') cid = tag.v_str;
                });

                if (tid && cid) {
                    lastEventContext.teamId = tid;
                    lastEventContext.channelId = cid;
                    lastEventContext.ts = Date.now();
                }
            }
        });
    } catch (e) {
        // Ignore JSON parse errors or other issues
    }
}

// Intercept fetch
const originalFetch = window.fetch;
window.fetch = function(input, init) {
    if (init && init.body) {
        // Clone body is tricky with streams, but Slack's telemetry is usually string/json
        if (typeof init.body === 'string') {
            processTelemetryBody(init.body);
        }
    }

    const responsePromise = originalFetch.apply(this, arguments);
    try {
        responsePromise.then(response => {
            const url = (typeof input === 'string' ? input : input && input.url) || response.url || '';
            if (isBadgeCountsUrl(url)) {
                rememberBadgeCountsRequest(
                    url,
                    (init && init.method) || (input && input.method) || 'GET',
                    init && init.body,
                    (init && init.headers) || (input && input.headers)
                );
                response.clone().text().then(text => processBadgeCountsResponse(url, text)).catch(() => {});
            } else if (isWorkspaceInitUrl(url)) {
                response.clone().text().then(text => processWorkspaceInitResponse(url, text)).catch(() => {});
            }
        });
    } catch (_) {}
    return responsePromise;
};

// Intercept navigator.sendBeacon
const originalSendBeacon = navigator.sendBeacon;
navigator.sendBeacon = function(url, data) {
    if (data && typeof data === 'string') {
        processTelemetryBody(data);
    }
    return originalSendBeacon.apply(this, arguments);
};

// Intercept XMLHttpRequest (just in case they use it for some calls)
const originalXHROpen = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function(method, url) {
    this.__zlackMethod = method;
    this.__zlackUrl = url;
    this.__zlackHeaders = {};
    return originalXHROpen.apply(this, arguments);
};

const originalXHRSetRequestHeader = XMLHttpRequest.prototype.setRequestHeader;
XMLHttpRequest.prototype.setRequestHeader = function(header, value) {
    try {
        if (!this.__zlackHeaders) this.__zlackHeaders = {};
        this.__zlackHeaders[header] = value;
    } catch (_) {}
    return originalXHRSetRequestHeader.apply(this, arguments);
};

const originalXHRScan = XMLHttpRequest.prototype.send;
XMLHttpRequest.prototype.send = function(body) {
    if (body && typeof body === 'string') {
        processTelemetryBody(body);
    }
    try {
        const requestUrl = this.__zlackUrl || '';
        if (isBadgeCountsUrl(requestUrl)) {
            rememberBadgeCountsRequest(requestUrl, this.__zlackMethod || 'GET', body, this.__zlackHeaders);
        }
        this.addEventListener('load', () => {
            const url = this.responseURL || this.__zlackUrl || '';
            if (!isBadgeCountsUrl(url) && !isWorkspaceInitUrl(url)) return;
            if (this.responseType === '' || this.responseType === 'text') {
                if (isBadgeCountsUrl(url)) processBadgeCountsResponse(url, this.responseText);
                else processWorkspaceInitResponse(url, this.responseText);
            } else if (this.responseType === 'json') {
                if (isBadgeCountsUrl(url)) processBadgeCountsResponse(url, this.response);
                else processWorkspaceInitResponse(url, this.response);
            }
        });
    } catch (_) {}
    return originalXHRScan.apply(this, arguments);
};


// 3. Shim Notification API
const ZlackNotification = class {
  constructor(title, options, ...args) {
    this.title = title;
    this.options = options || {};
    this.clickHandlers = [];
    
    // Store as global pending notification
    window.__ZlackPendingNotification = this;

    const createdAt = Date.now();
    const originalBody = this.options.body || '';
    const notifyTitle = typeof title === 'string' ? title : 'New Message';

    try {
      if (window.__TAURI__) {
          // Wait briefly so a slightly-delayed telemetry beacon can still land,
          // then only adopt the captured context if it belongs to THIS notification.
          setTimeout(() => {
              const age = createdAt - lastEventContext.ts; // >0 captured before, <0 after
              const fresh = lastEventContext.ts > 0
                  && age <= CONTEXT_FRESHNESS_MS
                  && age >= -NOTIFY_DELAY_MS;
              const teamId = fresh ? (lastEventContext.teamId || 'unknown') : 'unknown';
              const channelId = fresh ? (lastEventContext.channelId || 'unknown') : 'unknown';

              tauriInvoke('notify', {
                title: notifyTitle,
                body: originalBody,
                teamId: teamId,
                channelId: channelId
              }).catch(() => {});
          }, NOTIFY_DELAY_MS);
      }
    } catch (_) {}
  }

  static get permission() { return "granted"; }
  static requestPermission(cb) {
    if (cb) cb("granted");
    return Promise.resolve("granted");
  }

  // Support addEventListener for 'click' (dummy mostly, as we rely on native focus)
  addEventListener(type, listener) {
    if (type === 'click' && typeof listener === 'function') {
        this.clickHandlers.push(listener);
    }
  }
  
  removeEventListener(type, listener) {
    if (type === 'click') {
        this.clickHandlers = this.clickHandlers.filter(l => l !== listener);
    }
  }

  close() {}
};

// Force the shim to stay
Object.defineProperty(window, 'Notification', {
    value: ZlackNotification,
    writable: false,
    configurable: false
});


// 3.5 Unread / mention badge bridge -> native tray icon + window title.
// Badge state comes from Slack count snapshots and WebSocket events, not from
// document.title. The title observer below only keeps the native window title text
// in sync, with Slack's own unread markers stripped before Rust adds a "!" prefix.
const BADGE_COUNTS_URL_RE = /\/api\/client\.counts\b|\/cache\/[^/]+\/users\/counts\b/i;
const WORKSPACE_INIT_URL_RE = /\/api\/client\.init\b/i;
const WORKSPACE_CACHE_KEY = 'ZLACK_WORKSPACES';
const BADGE_TITLE_UNREAD_MARKER = /^\s*[\*•●·⁕∗∘◦]+/;
const BADGE_TITLE_COUNT_MARKER = /^\s*\((\d+)\)/;
const BADGE_STATE_PRIORITY = { none: 0, unread: 1, mention: 2 };
const BADGE_COUNTS_POLL_MS = 30000;
const BADGE_MENTION_COUNT_KEYS = [
    'dm_count',
    'mention_count_display',
    'num_mentions_display',
    'mention_count',
    'mentions_count',
    'highlight_count',
    'highlight_cnt',
    'badge_count',
];
const BADGE_UNREAD_COUNT_KEYS = [
    'unread_count_display',
    'unread_count',
    'unread_count_sum',
    'unread_cnt',
];
const BADGE_MENTION_BOOL_KEYS = ['has_mentions', 'has_highlights', 'is_mentioned', 'mentioned'];
const BADGE_UNREAD_BOOL_KEYS = ['unread', 'is_unread', 'has_unreads', 'has_unread'];

let badgeSelfUserId = null;
let badgeWindowTitle = 'Zlack';
let badgeLastSent = { state: null, title: null };
let lastWorkspaceSwitch = { team: null, ts: 0 };
let didRegisterCachedWorkspaces = false;
let workspaceOrder = [];
let badgeLastCountsRequest = null;
let badgeCountsPollInFlight = false;
let workspaceButtonsStatus = { active: null, workspaces: [] };
let workspaceButtonsRequestInFlight = false;
let workspaceButtonsRenderScheduled = false;
let workspaceMetaLastSent = '';
const badgeCountSnapshots = new Map();
const badgeRealtimeStates = new Map();
const workspaceTeamByDomain = new Map();
const workspaceTeamByName = new Map();

function readWorkspaceCache() {
    try {
        const parsed = JSON.parse(localStorage.getItem(WORKSPACE_CACHE_KEY) || '[]');
        return Array.isArray(parsed) ? parsed : [];
    } catch (_) {
        return [];
    }
}

function writeWorkspaceCache(entries) {
    try {
        localStorage.setItem(WORKSPACE_CACHE_KEY, JSON.stringify(entries));
    } catch (_) {}
}

function cacheWorkspace(team, { domain = null, name = null } = {}) {
    if (!/^T[A-Z0-9]{7,}$/.test(team || '')) return;
    const normalizedDomain = domain ? normalizeWorkspaceDomain(domain) : null;
    const normalizedName = name ? normalizeWorkspaceName(name) : null;
    const existing = readWorkspaceCache().find(entry => entry && entry.team === team) || {};
    let entries = readWorkspaceCache().filter(entry => {
        if (!entry || !entry.team) return false;
        if (entry.team === team) return false;
        if (normalizedDomain && normalizeWorkspaceDomain(entry.domain) === normalizedDomain) return false;
        if (normalizedName && normalizeWorkspaceName(entry.name) === normalizedName) return false;
        return true;
    });
    entries.push({
        team,
        domain: normalizedDomain || normalizeWorkspaceDomain(existing.domain) || null,
        name: normalizedName || normalizeWorkspaceName(existing.name) || null,
    });
    writeWorkspaceCache(entries);
}

function readLocalConfigWorkspaces() {
    try {
        const config = JSON.parse(localStorage.getItem('localConfig_v2') || '{}');
        const teams = config && config.teams && typeof config.teams === 'object' ? config.teams : {};
        return Object.values(teams)
            .map(team => ({
                team: team && team.id,
                domain: team && (team.domain || team.url),
                name: team && team.name,
            }))
            .filter(entry => /^T[A-Z0-9]{7,}$/.test(entry.team || ''));
    } catch (_) {
        return [];
    }
}

function setWorkspaceOrder(teams) {
    const seen = new Set();
    workspaceOrder = teams.filter(team => {
        if (!/^T[A-Z0-9]{7,}$/.test(team || '') || seen.has(team)) return false;
        seen.add(team);
        return true;
    });
    return workspaceOrder;
}

function loadWorkspaceCache() {
    const merged = new Map();
    // localConfig_v2 is Slack's own account/workspace list, so prefer its order.
    for (const entry of readLocalConfigWorkspaces().concat(readWorkspaceCache())) {
        if (!entry || !/^T[A-Z0-9]{7,}$/.test(entry.team || '')) continue;
        const existing = merged.get(entry.team) || {};
        merged.set(entry.team, {
            team: entry.team,
            domain: normalizeWorkspaceDomain(entry.domain) || normalizeWorkspaceDomain(existing.domain) || null,
            name: normalizeWorkspaceName(entry.name) || normalizeWorkspaceName(existing.name) || null,
        });
    }

    const validEntries = Array.from(merged.values());
    writeWorkspaceCache(validEntries);

    const teams = [];
    for (const entry of validEntries) {
        teams.push(entry.team);
        rememberWorkspaceAlias(entry.team, entry.domain, true, false);
        rememberWorkspaceAlias(entry.team, entry.name, false, false);
    }
    return setWorkspaceOrder(teams);
}

function createTauriCallback(callback, once = true) {
    const id = window.crypto.getRandomValues(new Uint32Array(1))[0];
    const prop = `_${id}`;
    Object.defineProperty(window, prop, {
        value: value => {
            if (once) Reflect.deleteProperty(window, prop);
            return callback && callback(value);
        },
        writable: false,
        configurable: true,
    });
    return id;
}

function directTauriInvoke(command, args = {}) {
    return new Promise((resolve, reject) => {
        const callback = createTauriCallback(value => {
            resolve(value);
            Reflect.deleteProperty(window, `_${error}`);
        });
        const error = createTauriCallback(value => {
            reject(value);
            Reflect.deleteProperty(window, `_${callback}`);
        });
        window.__TAURI_IPC__({ cmd: command, callback, error, ...args });
    });
}

function currentTauriInvoke() {
    // Only treat IPC as ready once __TAURI_IPC__ exists. Prefer the public global
    // Tauri API; __TAURI_INVOKE__ can queue forever in some remote-domain cases.
    if (typeof window.__TAURI_IPC__ !== 'function') return null;
    if (window.__TAURI__ && typeof window.__TAURI__.invoke === 'function') {
        return (command, args) => window.__TAURI__.invoke(command, args);
    }
    if (window.__TAURI__ && window.__TAURI__.tauri && typeof window.__TAURI__.tauri.invoke === 'function') {
        return (command, args) => window.__TAURI__.tauri.invoke(command, args);
    }
    if (typeof window.__TAURI_INVOKE__ === 'function') return window.__TAURI_INVOKE__;
    return directTauriInvoke;
}

function tauriInvoke(command, args, timeoutMs = 10000) {
    const started = Date.now();
    return new Promise((resolve, reject) => {
        function attempt() {
            const invoke = currentTauriInvoke();
            if (invoke) {
                Promise.resolve(invoke(command, args)).then(resolve, reject);
            } else if (Date.now() - started < timeoutMs) {
                setTimeout(attempt, 50);
            } else {
                reject(new Error('Tauri IPC is not available'));
            }
        }
        attempt();
    });
}

function isBadgeCountsUrl(url) {
    return BADGE_COUNTS_URL_RE.test(url || '');
}

function isWorkspaceInitUrl(url) {
    return WORKSPACE_INIT_URL_RE.test(url || '');
}

function badgeHeadersObject(headers) {
    const out = {};
    try {
        if (!headers) return out;
        if (headers instanceof Headers) {
            headers.forEach((value, key) => { out[key] = value; });
        } else if (Array.isArray(headers)) {
            headers.forEach(([key, value]) => { out[key] = value; });
        } else if (typeof headers === 'object') {
            Object.assign(out, headers);
        }
    } catch (_) {}
    return out;
}

function rememberBadgeCountsRequest(url, method = 'GET', body = null, headers = null) {
    if (!isBadgeCountsUrl(url)) return;
    try {
        const normalizedMethod = String(method || 'GET').toUpperCase();
        const normalizedBody = body instanceof URLSearchParams ? body.toString() : body;
        if (normalizedMethod !== 'GET' && typeof normalizedBody !== 'string') return;
        badgeLastCountsRequest = {
            url: new URL(url, window.location.href).href,
            method: normalizedMethod,
            body: normalizedMethod === 'GET' ? null : normalizedBody,
            headers: badgeHeadersObject(headers),
        };
    } catch (_) {}
}

function pollBadgeCounts() {
    if (!badgeLastCountsRequest || badgeCountsPollInFlight) return;
    const request = badgeLastCountsRequest;
    badgeCountsPollInFlight = true;

    const init = {
        method: request.method,
        credentials: 'include',
        cache: 'no-store',
        headers: request.headers,
    };
    if (request.method !== 'GET') init.body = request.body;

    originalFetch(request.url, init)
        .then(response => response.text())
        .then(text => processBadgeCountsResponse(request.url, text))
        .catch(() => {})
        .finally(() => { badgeCountsPollInFlight = false; });
}

function badgeCount(value) {
    const n = typeof value === 'number' ? value : parseInt(value, 10);
    return Number.isFinite(n) ? n : 0;
}

function badgeMaxCount(obj, keys) {
    if (!obj || typeof obj !== 'object') return 0;
    let max = 0;
    for (const key of keys) {
        if (Object.prototype.hasOwnProperty.call(obj, key)) {
            max = Math.max(max, badgeCount(obj[key]));
        }
    }
    return max;
}

function badgeChannelId(obj) {
    if (!obj || typeof obj !== 'object') return null;
    if (typeof obj.channel === 'string') return obj.channel;
    if (obj.channel && typeof obj.channel === 'object') return obj.channel.id || obj.channel.channel_id || null;
    if (typeof obj.channel_id === 'string') return obj.channel_id;
    if (typeof obj.id === 'string' && /^[CDG][A-Z0-9]{7,}$/.test(obj.id)) return obj.id;
    return null;
}

function badgeTeamId(obj) {
    if (!obj || typeof obj !== 'object') return null;
    if (typeof obj.team === 'string') return obj.team;
    if (obj.team && typeof obj.team === 'object') return obj.team.id || obj.team.team_id || null;
    if (typeof obj.team_id === 'string') return obj.team_id;
    if (typeof obj.id === 'string' && /^T[A-Z0-9]{7,}$/.test(obj.id)) return obj.id;
    return null;
}

function badgeContextFromKey(key, context) {
    const next = { ...context };
    if (/^[CDG][A-Z0-9]{7,}$/.test(key)) next.channel = key;
    if (/^T[A-Z0-9]{7,}$/.test(key)) next.team = key;
    return next;
}

function normalizeWorkspaceDomain(value) {
    return String(value || '')
        .trim()
        .toLowerCase()
        .replace(/^https?:\/\//, '')
        .replace(/\/.*$/, '')
        .replace(/\/+$/, '');
}

function normalizeWorkspaceName(value) {
    return String(value || '').trim().toLowerCase().replace(/\s+/g, ' ');
}

function rememberWorkspaceAlias(team, value, isDomain, persist = true) {
    if (!team || !value) return;
    if (isDomain) {
        const domain = normalizeWorkspaceDomain(value);
        if (!domain) return;
        workspaceTeamByDomain.set(domain, team);
        if (!domain.endsWith('.slack.com')) workspaceTeamByDomain.set(`${domain}.slack.com`, team);
        if (persist) cacheWorkspace(team, { domain });
    } else {
        const name = normalizeWorkspaceName(value);
        if (name) {
            workspaceTeamByName.set(name, team);
            if (persist) cacheWorkspace(team, { name });
        }
    }
}

function collectWorkspaceTeams(value, out = new Set(), depth = 0) {
    if (!value || depth > 8) return out;
    if (Array.isArray(value)) {
        value.forEach(item => collectWorkspaceTeams(item, out, depth + 1));
        return out;
    }
    if (typeof value !== 'object') return out;

    const team = badgeTeamId(value);
    if (team) {
        out.add(team);
        rememberWorkspaceAlias(team, value.name || value.team_name || value.label, false);
        rememberWorkspaceAlias(team, value.domain || value.url || value.team_url || value.enterprise_url, true);
        if (value.team && typeof value.team === 'object') {
            rememberWorkspaceAlias(team, value.team.name || value.team.team_name, false);
            rememberWorkspaceAlias(team, value.team.domain || value.team.url || value.team.team_url, true);
        }
    }
    for (const [key, child] of Object.entries(value)) {
        if (/^T[A-Z0-9]{7,}$/.test(key)) out.add(key);
        if (child && typeof child === 'object') collectWorkspaceTeams(child, out, depth + 1);
    }
    return out;
}

function processWorkspaceInitResponse(url, textOrObject) {
    if (!isWorkspaceInitUrl(url)) return;
    try {
        const data = typeof textOrObject === 'string' ? JSON.parse(textOrObject) : textOrObject;
        if (!data || typeof data !== 'object') return;
        if (data.self && data.self.id) badgeSelfUserId = data.self.id;
        const route = badgeCurrentRoute();
        const active = route.team || badgeTeamId(data.team) || badgeTeamId(data);
        const teams = Array.from(collectWorkspaceTeams(data.workspaces || data))
            .filter(team => /^T[A-Z0-9]{7,}$/.test(team));
        if (active) {
            if (!teams.includes(active)) teams.push(active);
            if (teams.length > workspaceOrder.length) setWorkspaceOrder(teams);
            if (data.team) {
                rememberWorkspaceAlias(active, data.team.name || data.team.team_name, false);
                rememberWorkspaceAlias(active, data.team.domain || data.team.url || data.team.team_url, true);
            }
        }
        if (teams.length) {
            tauriInvoke('register_workspaces', { teams, active }).catch(() => {});
        }
    } catch (_) {}
}

function badgeIsDmChannel(channel, path = '') {
    return (!!channel && String(channel).startsWith('D')) || /(^|\.)(ims|mpims)(\.|\[|$)/i.test(path);
}

function badgeCleanTitle(title) {
    return (title || 'Zlack')
        .replace(BADGE_TITLE_COUNT_MARKER, '')
        .replace(BADGE_TITLE_UNREAD_MARKER, '')
        .replace(/^\s*!+\s*/, '')
        .trim() || 'Zlack';
}

function badgeCurrentRoute() {
    const m = window.location.pathname.match(/\/client\/([^/]+)(?:\/([^/?#]+))?/);
    return m ? { team: m[1], channel: m[2] || null } : { team: null, channel: null };
}

function badgeStateFromCounts(obj, path = '', channelHint = null) {
    if (!obj || typeof obj !== 'object') return { state: 'none', hasCounts: false };

    const channel = badgeChannelId(obj) || channelHint;
    const mentionCount = badgeMaxCount(obj, BADGE_MENTION_COUNT_KEYS);
    const unreadCount = badgeMaxCount(obj, BADGE_UNREAD_COUNT_KEYS);
    const hasExplicitCount = BADGE_MENTION_COUNT_KEYS
        .concat(BADGE_UNREAD_COUNT_KEYS)
        .some(key => Object.prototype.hasOwnProperty.call(obj, key));
    const hasMentionBool = BADGE_MENTION_BOOL_KEYS.some(key => obj[key] === true || obj[key] === 'true');
    const hasUnreadBool = BADGE_UNREAD_BOOL_KEYS.some(key => obj[key] === true || obj[key] === 'true');

    if (mentionCount > 0 || hasMentionBool) return { state: 'mention', hasCounts: true };
    if (unreadCount > 0 || hasUnreadBool) {
        return {
            state: badgeIsDmChannel(channel, path) ? 'mention' : 'unread',
            hasCounts: true,
        };
    }
    if (hasExplicitCount) return { state: 'none', hasCounts: true };

    return { state: 'none', hasCounts: false };
}

function badgeAggregateState() {
    let state = 'none';
    for (const snapshot of badgeCountSnapshots.values()) {
        for (const item of snapshot.values()) {
            if (BADGE_STATE_PRIORITY[item.state] > BADGE_STATE_PRIORITY[state]) state = item.state;
        }
    }
    for (const item of badgeRealtimeStates.values()) {
        if (BADGE_STATE_PRIORITY[item.state] > BADGE_STATE_PRIORITY[state]) state = item.state;
    }
    return state;
}

function badgeSetRealtime(key, item) {
    if (!key) return;
    if (!item || item.state === 'none') badgeRealtimeStates.delete(key);
    else badgeRealtimeStates.set(key, item);
}

function badgePush() {
    badgeClearCurrentVisibleChannel();
    const state = badgeAggregateState();
    const title = badgeWindowTitle || badgeCleanTitle(document.title) || 'Zlack';
    if (state === badgeLastSent.state && title === badgeLastSent.title) return;
    badgeLastSent = { state, title };
    try {
        window.dispatchEvent(new Event('zlack-badge-state-changed'));
    } catch (_) {}
    try {
        // The taskbar overlay is a dot only, so no unread count is needed.
        const team = badgeCurrentRoute().team;
        tauriInvoke('update_badge', { state, title, count: null, team }).catch(() => {});
    } catch (_) {}
}

function badgeCollectSnapshot(value, path = '', out = [], depth = 0, context = {}) {
    if (!value || depth > 8) return out;
    if (Array.isArray(value)) {
        value.forEach((item, index) => badgeCollectSnapshot(item, `${path}[${index}]`, out, depth + 1, context));
        return out;
    }
    if (typeof value !== 'object') return out;

    const channel = badgeChannelId(value) || context.channel || null;
    const team = badgeTeamId(value) || context.team || null;
    const nextContext = { channel, team };
    const countState = badgeStateFromCounts(value, path, channel);
    if (channel && countState.hasCounts && countState.state !== 'none') {
        out.push({
            key: `${team || 'team'}:${channel}`,
            state: countState.state,
        });
    }

    for (const [key, child] of Object.entries(value)) {
        if (child && typeof child === 'object') {
            badgeCollectSnapshot(child, path ? `${path}.${key}` : key, out, depth + 1, badgeContextFromKey(key, nextContext));
        }
    }
    return out;
}

function badgeSourceKey(url, data) {
    const route = badgeCurrentRoute();
    const cacheMatch = String(url || '').match(/\/cache\/([^/]+)\/users\/counts\b/i);
    if (cacheMatch) return `users-counts:${cacheMatch[1]}`;
    return `client-counts:${badgeTeamId(data) || route.team || window.location.hostname}`;
}

function processBadgeCountsResponse(url, textOrObject) {
    if (!isBadgeCountsUrl(url)) return;
    try {
        const data = typeof textOrObject === 'string' ? JSON.parse(textOrObject) : textOrObject;
        if (!data || typeof data !== 'object') return;
        if (data.self && data.self.id) badgeSelfUserId = data.self.id;

        const snapshot = new Map();
        for (const item of badgeCollectSnapshot(data)) {
            snapshot.set(item.key, { state: item.state });
        }
        badgeCountSnapshots.set(badgeSourceKey(url, data), snapshot);
        badgePush();
    } catch (_) {}
}

function badgeClearChannel(obj) {
    const team = badgeTeamId(obj);
    const channel = badgeChannelId(obj);
    if (!channel) return false;

    let changed = false;
    if (badgeRealtimeStates.delete(`${team || 'team'}:${channel}`)) changed = true;
    for (const key of Array.from(badgeRealtimeStates.keys())) {
        if (key.endsWith(`:${channel}`) || key === channel) {
            badgeRealtimeStates.delete(key);
            changed = true;
        }
    }
    for (const snapshot of badgeCountSnapshots.values()) {
        if (snapshot.delete(`${team || 'team'}:${channel}`)) changed = true;
        for (const key of Array.from(snapshot.keys())) {
            if (key.endsWith(`:${channel}`) || key === channel) {
                snapshot.delete(key);
                changed = true;
            }
        }
    }
    return changed;
}

function badgeClearCurrentVisibleChannel() {
    const route = badgeCurrentRoute();
    if (!route.team || !route.channel || !document.hasFocus() || document.visibilityState === 'hidden') return false;
    return badgeClearChannel({ team_id: route.team, channel_id: route.channel });
}

function badgeClearCurrentVisibleChannelAndPush() {
    if (badgeClearCurrentVisibleChannel()) badgePush();
}

function badgeProcessRealtimeObject(obj, depth = 0, context = {}) {
    if (!obj || typeof obj !== 'object' || depth > 5) return;
    if (obj.self && obj.self.id) badgeSelfUserId = obj.self.id;

    // Reaction events include an `item` that looks like a message reference
    // (`{ type: 'message', channel, ts }`). Don't treat that reference as a
    // new unread message.
    if (obj.type === 'reaction_added' || obj.type === 'reaction_removed') return;

    const channel = badgeChannelId(obj) || context.channel || null;
    const team = badgeTeamId(obj) || context.team || null;
    const nextContext = { channel, team };

    if (Array.isArray(obj.clearing_data)) {
        obj.clearing_data.forEach(badgeClearChannel);
        badgeClearChannel({ ...obj, channel_id: channel, team_id: team });
        badgePush();
    }

    const countState = badgeStateFromCounts(obj, 'event', channel);
    if (channel && countState.hasCounts) {
        if (countState.state === 'none') {
            badgeClearChannel({ ...obj, team_id: team, channel_id: channel });
        } else {
            badgeSetRealtime(`${team || 'team'}:${channel}`, { state: countState.state });
        }
        badgePush();
    }

    if (obj.type === 'message' && channel) {
        const route = badgeCurrentRoute();
        const isVisibleChannel = document.hasFocus() && channel === route.channel && (!team || !route.team || team === route.team);
        if (!isVisibleChannel && !obj.subtype && (!badgeSelfUserId || obj.user !== badgeSelfUserId)) {
            const text = typeof obj.text === 'string' ? obj.text : '';
            const mentionedSelf = badgeSelfUserId && text.includes(`<@${badgeSelfUserId}>`);
            const state = (badgeIsDmChannel(channel) || mentionedSelf) ? 'mention' : 'unread';
            badgeSetRealtime(`${team || 'team'}:${channel}`, { state });
            badgePush();
        }
    }

    for (const [key, child] of Object.entries(obj)) {
        if (child && typeof child === 'object') {
            badgeProcessRealtimeObject(child, depth + 1, badgeContextFromKey(key, nextContext));
        }
    }
}

function processBadgeSocketMessage(data) {
    if (typeof data !== 'string') return;
    try {
        badgeProcessRealtimeObject(JSON.parse(data));
    } catch (_) {}
}

(function setupBadgeBridge() {
    if (window.WebSocket) {
        const OriginalWebSocket = window.WebSocket;
        window.WebSocket = function(url, protocols) {
            const ws = protocols === undefined ? new OriginalWebSocket(url) : new OriginalWebSocket(url, protocols);
            try {
                ws.addEventListener('message', event => processBadgeSocketMessage(event.data));
            } catch (_) {}
            return ws;
        };
        window.WebSocket.prototype = OriginalWebSocket.prototype;
        Object.setPrototypeOf(window.WebSocket, OriginalWebSocket);
    }

    function pushTitle() {
        badgeWindowTitle = badgeCleanTitle(document.title || 'Zlack');
        badgePush();
    }

    function injectUserCss(css) {
        if (!css || typeof css !== 'string') return;
        let style = document.getElementById('zlack-user-css');
        if (!style) {
            style = document.createElement('style');
            style.id = 'zlack-user-css';
            style.type = 'text/css';
            (document.head || document.documentElement).appendChild(style);
        }
        style.textContent = css;
    }

    function loadUserCss() {
        tauriInvoke('load_user_css').then(injectUserCss).catch(() => {});
    }

    function closestFromEventTarget(target, selector) {
        const el = target && (target.closest ? target : target.parentElement);
        return el && el.closest ? el.closest(selector) : null;
    }

    function findNativeWorkspaceButton() {
        return Array.from(document.querySelectorAll('button.p-account_switcher[data-qa="account_switcher_team_icon"], button[data-qa="account_switcher_team_icon"]'))
            .find(button => !button.closest('#zlack-workspace-switcher')) || null;
    }

    function workspaceFallbackName(team) {
        return team || 'Workspace';
    }

    function workspaceInitials(name, team) {
        const text = String(name || '').trim();
        if (text) {
            const initials = text.split(/\s+/).map(part => part[0]).join('').slice(0, 2).toUpperCase();
            if (initials) return initials;
        }
        return String(team || '?').replace(/^T/, '').slice(0, 2).toUpperCase() || '?';
    }

    function workspaceNameFromNativeButton(button, iconEl) {
        let name = iconEl?.getAttribute('aria-label') || '';
        if (!name) {
            const label = button?.getAttribute('aria-label') || '';
            const match = label.match(/\(([^()]+)\)\s*$/);
            name = match ? match[1] : label.replace(/^Switch workspaces…?\s*/i, '').trim();
        }
        return name || '';
    }

    function currentWorkspaceMeta() {
        const team = badgeCurrentRoute().team;
        const button = findNativeWorkspaceButton();
        if (!team || !button) return null;
        const iconEl = button.querySelector('[data-qa="team-icon"], .c-team_icon');
        const computed = iconEl ? window.getComputedStyle(iconEl) : null;
        const name = workspaceNameFromNativeButton(button, iconEl);
        const backgroundImage = (iconEl?.style?.backgroundImage || computed?.backgroundImage || '').trim();
        const backgroundColor = (iconEl?.style?.backgroundColor || computed?.backgroundColor || '').trim();
        const iconText = (iconEl?.textContent || '').trim() || workspaceInitials(name, team);
        return {
            team,
            name,
            iconImage: backgroundImage && backgroundImage !== 'none' ? backgroundImage : '',
            iconText,
            iconColor: backgroundColor && !/^rgba\(\s*0\s*,\s*0\s*,\s*0\s*,\s*0\s*\)$/i.test(backgroundColor) ? backgroundColor : '',
        };
    }

    function sendCurrentWorkspaceMeta() {
        const meta = currentWorkspaceMeta();
        if (!meta) return;
        const serialized = JSON.stringify(meta);
        if (serialized === workspaceMetaLastSent) return;
        workspaceMetaLastSent = serialized;
        tauriInvoke('update_workspace_meta', {
            team: meta.team,
            name: meta.name,
            iconImage: meta.iconImage,
            iconText: meta.iconText,
            iconColor: meta.iconColor,
        }).catch(() => {});
    }

    function workspaceInfoByTeam(team) {
        const workspaces = Array.isArray(workspaceButtonsStatus?.workspaces) ? workspaceButtonsStatus.workspaces : [];
        return workspaces.find(item => item && item.team === team) || null;
    }

    function workspaceBadgeState(team) {
        const statusBadge = workspaceInfoByTeam(team)?.badge || 'none';
        if (team === badgeCurrentRoute().team) {
            const localBadge = badgeAggregateState();
            return localBadge !== 'none' ? localBadge : statusBadge;
        }
        return statusBadge;
    }

    function setWorkspaceBadge(button, state) {
        button.style.setProperty('position', 'relative', 'important');
        let badge = button.querySelector(':scope > .zlack-workspace-badge');
        if (!badge) {
            badge = document.createElement('span');
            badge.className = 'zlack-workspace-badge';
            badge.setAttribute('aria-hidden', 'true');
            button.appendChild(badge);
        }
        Object.assign(badge.style, {
            display: state === 'mention' || state === 'unread' ? 'block' : 'none',
            position: 'absolute',
            top: '3px',
            right: '3px',
            width: '11px',
            height: '11px',
            borderRadius: '999px',
            border: '2px solid var(--sk_primary_background, #1a1d21)',
            background: state === 'mention' ? '#e01e5a' : '#36c5f0',
            zIndex: '2',
            pointerEvents: 'none',
        });
    }

    function applyWorkspaceIcon(button, info) {
        let icon = button.querySelector('.zlack-workspace-team-icon');
        if (!icon) {
            icon = document.createElement('i');
            icon.className = 'c-team_icon p-account_switcher__team_icon zlack-workspace-team-icon';
            icon.setAttribute('role', 'img');
            icon.setAttribute('aria-hidden', 'true');
            icon.setAttribute('data-qa', 'team-icon');
            button.appendChild(icon);
        }
        const name = info.name || workspaceFallbackName(info.team);
        const iconImage = info.icon_image || info.iconImage || '';
        icon.setAttribute('aria-label', name);
        Object.assign(icon.style, {
            height: '36px',
            width: '36px',
            minWidth: '36px',
            fontSize: '18px',
            lineHeight: '36px',
            textAlign: 'center',
            whiteSpace: 'nowrap',
            backgroundImage: iconImage || '',
            backgroundColor: info.icon_color || info.iconColor || '',
            backgroundSize: 'cover',
            backgroundPosition: 'center',
        });
        icon.textContent = iconImage ? '' : (info.icon_text || info.iconText || workspaceInitials(name, info.team));
    }

    function renderWorkspaceButtons() {
        const currentTeam = badgeCurrentRoute().team;
        const nativeButton = findNativeWorkspaceButton();
        if (!currentTeam || !nativeButton) return;

        const currentInfo = workspaceInfoByTeam(currentTeam) || currentWorkspaceMeta() || { team: currentTeam };
        const currentName = currentInfo.name || workspaceNameFromNativeButton(nativeButton, nativeButton.querySelector('[data-qa="team-icon"], .c-team_icon')) || workspaceFallbackName(currentTeam);
        nativeButton.classList.add('zlack-workspace-native-button');
        nativeButton.title = currentName;
        setWorkspaceBadge(nativeButton, workspaceBadgeState(currentTeam));

        const trigger = nativeButton.closest('.p-peek_trigger') || nativeButton.parentElement;
        if (!trigger || !trigger.parentElement) return;
        let container = document.getElementById('zlack-workspace-switcher');
        if (!container) {
            container = document.createElement('div');
            container.id = 'zlack-workspace-switcher';
            container.setAttribute('role', 'none');
        }
        container.style.display = 'contents';
        if (container.parentElement !== trigger.parentElement || container.previousElementSibling !== trigger) {
            trigger.after(container);
        }

        container.textContent = '';
        const workspaces = Array.isArray(workspaceButtonsStatus?.workspaces) ? workspaceButtonsStatus.workspaces : [];
        for (const info of workspaces) {
            if (!info || !info.team || info.team === currentTeam) continue;
            const name = info.name || workspaceFallbackName(info.team);
            const button = document.createElement('button');
            button.type = 'button';
            button.className = 'c-button-unstyled p-account_switcher zlack-workspace-native-button';
            button.dataset.zlackWorkspaceTeam = info.team;
            button.setAttribute('aria-label', name);
            button.title = name;
            applyWorkspaceIcon(button, info);
            setWorkspaceBadge(button, info.badge || 'none');
            container.appendChild(button);
        }
    }

    function scheduleWorkspaceButtonsRender() {
        if (workspaceButtonsRenderScheduled) return;
        workspaceButtonsRenderScheduled = true;
        requestAnimationFrame(() => {
            workspaceButtonsRenderScheduled = false;
            sendCurrentWorkspaceMeta();
            renderWorkspaceButtons();
        });
    }

    function applyWorkspaceStatus(status) {
        if (status && Array.isArray(status.workspaces)) workspaceButtonsStatus = status;
        scheduleWorkspaceButtonsRender();
    }

    function requestWorkspaceButtonStatus() {
        sendCurrentWorkspaceMeta();
        renderWorkspaceButtons();
        if (workspaceButtonsRequestInFlight) return;
        workspaceButtonsRequestInFlight = true;
        tauriInvoke('workspace_status', { current: badgeCurrentRoute().team }, 3000)
            .then(applyWorkspaceStatus)
            .catch(() => {})
            .finally(() => { workspaceButtonsRequestInFlight = false; });
    }

    function maybeHandleWorkspaceButtonEvent(event) {
        const button = closestFromEventTarget(event.target, '#zlack-workspace-switcher button[data-zlack-workspace-team]');
        if (!button) return;

        event.preventDefault();
        event.stopImmediatePropagation();

        if (!['pointerup', 'mouseup', 'click'].includes(event.type)) return;
        const team = button.dataset.zlackWorkspaceTeam;
        const currentTeam = badgeCurrentRoute().team;
        if (!team || !currentTeam || team === currentTeam) return;

        const now = Date.now();
        if (lastWorkspaceSwitch.team === team && now - lastWorkspaceSwitch.ts < 750) return;
        lastWorkspaceSwitch = { team, ts: now };
        closeWorkspaceSwitcherPopover();
        tauriInvoke('switch_workspace', { team, url: null }).catch(() => {});
    }

    window.__ZlackWorkspaceStatus = applyWorkspaceStatus;
    window.addEventListener('zlack-badge-state-changed', scheduleWorkspaceButtonsRender);

    function switchInfoFromSwitcherButton(target) {
        const button = closestFromEventTarget(target, 'button.p-team_switcher_menu__item, button[class*="team_switcher"], button[role="menuitemradio"]');
        if (!button) return null;

        const domain = normalizeWorkspaceDomain(button.querySelector('.p-account_switcher__row_url')?.textContent);
        let team = null;
        if (domain && workspaceTeamByDomain.has(domain)) team = workspaceTeamByDomain.get(domain);
        if (!team && domain && workspaceTeamByDomain.has(`${domain}.slack.com`)) team = workspaceTeamByDomain.get(`${domain}.slack.com`);

        let name = button.querySelector('.p-account_switcher__row_name')?.textContent;
        const labelledBy = button.getAttribute('aria-labelledby');
        if (!name && labelledBy) {
            const labelId = labelledBy.split(/\s+/).find(Boolean);
            name = labelId && document.getElementById(labelId)?.textContent;
        }
        if (!name) name = button.querySelector('[data-qa="team-icon"]')?.getAttribute('aria-label');
        name = normalizeWorkspaceName(name);
        if (!team) team = workspaceTeamByName.get(name) || null;

        if (team) return { team, url: null };

        // Do not intercept if we only know the workspace domain. Tauri v1 remote
        // IPC is exact-domain scoped, so loading a workspace subdomain such as
        // foo.slack.com breaks badge IPC. Let Slack do its normal switch; once that
        // workspace boots, client.init gives us the real team id for future swaps.
        return null;
    }

    function closeWorkspaceSwitcherPopover() {
        const popover = document.querySelector('.ReactModal__Content.c-popover__content, .ReactModal__Content.popover');
        const eventInit = {
            key: 'Escape',
            code: 'Escape',
            keyCode: 27,
            which: 27,
            bubbles: true,
            cancelable: true,
        };
        for (const target of [popover, document.activeElement, document, window].filter(Boolean)) {
            try {
                target.dispatchEvent(new KeyboardEvent('keydown', eventInit));
                target.dispatchEvent(new KeyboardEvent('keyup', eventInit));
            } catch (_) {}
        }
    }

    function maybeSwitchWorkspace(event) {
        const anchor = closestFromEventTarget(event.target, 'a[href]');
        let team = null;
        let switchUrl = null;

        if (anchor) {
            try {
                const url = new URL(anchor.href, window.location.href);
                if (/slack\.com$/i.test(url.hostname) || /\.slack\.com$/i.test(url.hostname)) {
                    const m = url.pathname.match(/^\/client\/([^/]+)(?:\/([^/?#]+))?/);
                    if (m) {
                        team = m[1];
                        switchUrl = url.href;
                    }
                }
            } catch (_) {}
        }

        if (!team) {
            const info = switchInfoFromSwitcherButton(event.target);
            if (info) {
                team = info.team;
                switchUrl = info.url;
            }
        }

        const currentTeam = badgeCurrentRoute().team;
        if (!team || !currentTeam || team === currentTeam) return;

        event.preventDefault();
        event.stopImmediatePropagation();

        const now = Date.now();
        if (lastWorkspaceSwitch.team === team && now - lastWorkspaceSwitch.ts < 750) {
            // pointerdown/mousedown/click can all fire for the same button press.
            // Keep suppressing Slack's native handler, but invoke Rust only once.
            return;
        }
        lastWorkspaceSwitch = { team, ts: now };

        closeWorkspaceSwitcherPopover();
        tauriInvoke('switch_workspace', { team, url: switchUrl }).catch(() => {});
    }

    function maybeSwitchWorkspaceShortcut(event) {
        if (!event.ctrlKey || event.altKey || event.metaKey || event.shiftKey || event.repeat) return;

        let digit = null;
        if (/^Digit[1-9]$/.test(event.code || '')) digit = event.code.slice(5);
        else if (/^Numpad[1-9]$/.test(event.code || '')) digit = event.code.slice(6);
        else if (/^[1-9]$/.test(event.key || '')) digit = event.key;
        if (!digit) return;

        const teams = workspaceOrder.length ? workspaceOrder : loadWorkspaceCache();
        const team = teams[parseInt(digit, 10) - 1];
        if (!team) return;

        event.preventDefault();
        event.stopImmediatePropagation();

        const currentTeam = badgeCurrentRoute().team;
        if (team === currentTeam) return;

        const now = Date.now();
        if (lastWorkspaceSwitch.team === team && now - lastWorkspaceSwitch.ts < 750) return;
        lastWorkspaceSwitch = { team, ts: now };

        tauriInvoke('switch_workspace', { team, url: null }).catch(() => {});
    }

    function registerCachedWorkspacesWhenReady() {
        if (didRegisterCachedWorkspaces) return;
        const active = badgeCurrentRoute().team;
        if (!active) return;
        const cachedTeams = loadWorkspaceCache();
        if (!cachedTeams.length) return;
        didRegisterCachedWorkspaces = true;
        tauriInvoke('register_workspaces', { teams: cachedTeams, active }).catch(() => {});
    }

    function start() {
        const head = document.querySelector('head');
        if (head) {
            new MutationObserver(pushTitle).observe(head, {
                subtree: true,
                childList: true,
                characterData: true,
            });
        }
        const workspaceObserverRoot = document.body || document.documentElement;
        if (workspaceObserverRoot) {
            const nativeButtonSelector = 'button.p-account_switcher[data-qa="account_switcher_team_icon"], button[data-qa="account_switcher_team_icon"]';
            new MutationObserver(records => {
                const changedWorkspaceButton = records.some(record => {
                    const target = record.target;
                    if (target && target.closest && target.closest('#zlack-workspace-switcher')) return false;
                    return Array.from(record.addedNodes).concat(Array.from(record.removedNodes)).some(node => {
                        if (!node || node.nodeType !== 1) return false;
                        return node.matches?.(nativeButtonSelector) || node.querySelector?.(nativeButtonSelector);
                    });
                });
                if (changedWorkspaceButton) scheduleWorkspaceButtonsRender();
            }).observe(workspaceObserverRoot, { childList: true, subtree: true });
        }
        document.addEventListener('pointerdown', maybeHandleWorkspaceButtonEvent, true);
        document.addEventListener('mousedown', maybeHandleWorkspaceButtonEvent, true);
        document.addEventListener('pointerup', maybeHandleWorkspaceButtonEvent, true);
        document.addEventListener('mouseup', maybeHandleWorkspaceButtonEvent, true);
        document.addEventListener('click', maybeHandleWorkspaceButtonEvent, true);
        document.addEventListener('pointerdown', maybeSwitchWorkspace, true);
        document.addEventListener('mousedown', maybeSwitchWorkspace, true);
        document.addEventListener('mouseup', maybeSwitchWorkspace, true);
        document.addEventListener('click', maybeSwitchWorkspace, true);
        document.addEventListener('keydown', maybeSwitchWorkspaceShortcut, true);
        document.addEventListener('pointerup', badgeClearCurrentVisibleChannelAndPush, true);
        document.addEventListener('keyup', badgeClearCurrentVisibleChannelAndPush, true);
        document.addEventListener('visibilitychange', badgeClearCurrentVisibleChannelAndPush, true);
        window.addEventListener('focus', badgeClearCurrentVisibleChannelAndPush, true);
        window.addEventListener('popstate', badgeClearCurrentVisibleChannelAndPush, true);
        setInterval(() => {
            badgeClearCurrentVisibleChannelAndPush();
            registerCachedWorkspacesWhenReady();
            pushTitle();
        }, 3000);
        setInterval(pollBadgeCounts, BADGE_COUNTS_POLL_MS);
        loadUserCss();
        registerCachedWorkspacesWhenReady();
        requestWorkspaceButtonStatus();
        pushTitle();
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', start);
    } else {
        start();
    }
})();


// 4. Intercept links/new windows.
// External links still open in the OS browser. Slack app/permalink links that
// Slack marks as a new tab are retargeted to this WebView so Slack's own router
// can handle them without a full app reload when possible.
const ZLACK_SLACK_HOST_RE = /(^|\.)slack\.com$/i;

function zlackUrl(href) {
    try {
        return new URL(href, window.location.href);
    } catch (_) {
        return null;
    }
}

function zlackIsHttpUrl(url) {
    return url && (url.protocol === 'http:' || url.protocol === 'https:');
}

function zlackIsSlackUrl(url) {
    return zlackIsHttpUrl(url) && ZLACK_SLACK_HOST_RE.test(url.hostname || '');
}

function zlackIsSlackAppPath(pathname) {
    return /^\/(?:client|archives)(?:\/|$)/i.test(pathname || '');
}

function zlackTeamForSlackUrl(url) {
    const queryTeam = url.searchParams.get('team') || url.searchParams.get('team_id');
    if (/^T[A-Z0-9]{7,}$/.test(queryTeam || '')) return queryTeam;

    const clientMatch = url.pathname.match(/^\/client\/([^/?#]+)/i);
    if (/^T[A-Z0-9]{7,}$/.test(clientMatch?.[1] || '')) return clientMatch[1];

    const domain = normalizeWorkspaceDomain(url.hostname);
    return workspaceTeamByDomain.get(domain) || badgeCurrentRoute().team || null;
}

function zlackSlackAppHref(href) {
    const url = zlackUrl(href);
    if (!zlackIsSlackUrl(url) || !zlackIsSlackAppPath(url.pathname)) return null;

    if (/^\/client(?:\/|$)/i.test(url.pathname)) return url.href;

    const team = zlackTeamForSlackUrl(url);
    const archiveMessageMatch = url.pathname.match(/^\/archives\/([^/?#]+)\/p(\d{10})(\d{6})/i);
    if (archiveMessageMatch && team) {
        const channel = archiveMessageMatch[1];
        const ts = `${archiveMessageMatch[2]}${archiveMessageMatch[3]}`;
        const appUrl = new URL(`https://app.slack.com/client/${encodeURIComponent(team)}/${encodeURIComponent(channel)}/thread/${encodeURIComponent(channel)}-p${ts}`);
        appUrl.search = url.search;
        appUrl.hash = url.hash;
        return appUrl.href;
    }

    const archiveChannelMatch = url.pathname.match(/^\/archives\/([^/?#]+)/i);
    if (archiveChannelMatch && team) {
        const channel = archiveChannelMatch[1];
        const appUrl = new URL(`https://app.slack.com/client/${encodeURIComponent(team)}/${encodeURIComponent(channel)}`);
        appUrl.search = url.search;
        appUrl.hash = url.hash;
        return appUrl.href;
    }

    return url.href;
}

function zlackRetargetSlackAnchor(anchor) {
    const appHref = zlackSlackAppHref(anchor.href);
    if (!appHref) return false;

    const oldHref = anchor.getAttribute('href');
    const oldTarget = anchor.getAttribute('target');
    anchor.setAttribute('href', appHref);
    anchor.removeAttribute('target');

    setTimeout(() => {
        if (oldHref == null) anchor.removeAttribute('href');
        else anchor.setAttribute('href', oldHref);
        if (oldTarget == null) anchor.removeAttribute('target');
        else anchor.setAttribute('target', oldTarget);
    }, 0);

    return true;
}

function zlackOpenSlackUrlInCurrentWindow(href) {
    const appHref = zlackSlackAppHref(href);
    if (!appHref) return false;

    const anchor = document.createElement('a');
    anchor.href = appHref;
    anchor.style.display = 'none';
    anchor.setAttribute('data-zlack-synthetic-link', 'true');
    (document.body || document.documentElement).appendChild(anchor);

    const event = new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        view: window,
    });
    const notCanceled = anchor.dispatchEvent(event);
    anchor.remove();

    if (notCanceled && !event.defaultPrevented && window.location.href !== appHref) {
        window.location.href = appHref;
    }
    return true;
}

window.__ZlackNavigateSlackUrl = zlackOpenSlackUrlInCurrentWindow;

const originalWindowOpen = window.open;
window.open = function(url, target, features) {
    const href = typeof url === 'string' ? url : (url && String(url));
    const parsed = href ? zlackUrl(href) : null;

    if (zlackSlackAppHref(href)) {
        zlackOpenSlackUrlInCurrentWindow(href);
        return null;
    }

    if (zlackIsHttpUrl(parsed) && !zlackIsSlackUrl(parsed)) {
        if (window.__TAURI__) window.__TAURI__.shell.open(parsed.href);
        return null;
    }

    return originalWindowOpen.apply(this, arguments);
};

document.addEventListener('click', (e) => {
    const target = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!target || !target.href) return;

    const href = target.href;
    const parsed = zlackUrl(href);
    const isExternal = zlackIsHttpUrl(parsed) && !zlackIsSlackUrl(parsed);

    if (isExternal) {
        e.preventDefault();
        e.stopImmediatePropagation();
        if (window.__TAURI__) window.__TAURI__.shell.open(parsed.href);
        return;
    }

    if (target.target === '_blank') {
        // Do not prevent default here. Just remove the new-tab behavior for this
        // event turn and let Slack's own click handling do the in-app navigation.
        zlackRetargetSlackAnchor(target);
    }
}, true); // Capture phase to ensure we get it before Slack




