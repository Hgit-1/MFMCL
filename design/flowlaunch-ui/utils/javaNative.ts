import { invoke } from '@tauri-apps/api/core';
import { getSettings, isDesktopShell, NativeUnavailableError, prefsToSettings, type LauncherSettings } from './native';
import { prefsFromSettings, type LauncherPrefs } from './launcherPrefs';
import type { Language } from './i18n';
import { LocalizedError, loc, localizedTextOf, renderLocalized } from './i18n/localized';
import {
  cacheJavaPreferences, DEFAULT_INSTANCE_JAVA, INSTANCE_JAVA_KEY, JAVA_LIST_KEY, javaSelectionRequest,
  javaRuleForPersistence, parseFriendlyRange, readLegacyJavaPreferences, type InstanceJavaSetting, type JavaRuntime,
  type JavaSelectionRequest, type NativeJavaSelection, type JavaPreferences, type JavaPreferencesUpdate,
} from './java';

export async function selectInstanceJava(instanceId: string, selection?: JavaSelectionRequest): Promise<NativeJavaSelection> {
  if (!isDesktopShell()) throw new NativeUnavailableError('select_instance_java');
  return invoke<NativeJavaSelection>('select_instance_java', { instanceId, selection: selection ?? null });
}

export async function installInstanceJava(instanceId: string, selection?: JavaSelectionRequest): Promise<JavaRuntime> {
  if (!isDesktopShell()) throw new NativeUnavailableError('install_instance_java');
  await flushPendingJavaPreferences(instanceId);
  return invoke<JavaRuntime>('install_instance_java', { instanceId, selection: selection ?? null });
}

export async function cancelJavaInstall(instanceId: string): Promise<void> {
  if (!isDesktopShell()) throw new NativeUnavailableError('cancel_java_install');
  await invoke('cancel_java_install', { instanceId });
}

let migration: Promise<void> | undefined;
let operations: Promise<unknown> = Promise.resolve();

function serialized<T>(operation: () => Promise<T>): Promise<T> {
  const result = operations.then(operation);
  operations = result.then(() => undefined, () => undefined);
  return result;
}

export interface JavaRuleDraft {
  rule: InstanceJavaSetting;
  confirmed: InstanceJavaSetting;
  revision: number;
  dirty: boolean;
  error?: string;
}

interface DraftEntry {
  snapshot: JavaRuleDraft;
  listeners: Set<() => void>;
  timer?: ReturnType<typeof setTimeout>;
  pending?: { revision: number; promise: Promise<void> };
  failure?: Error;
}

const drafts = new Map<string, DraftEntry>();
const ruleKey = (rule: InstanceJavaSetting) => JSON.stringify(javaRuleForPersistence(rule));

function entryFor(instanceId: string): DraftEntry {
  let entry = drafts.get(instanceId);
  if (!entry) {
    entry = { snapshot: { rule: { ...DEFAULT_INSTANCE_JAVA }, confirmed: { ...DEFAULT_INSTANCE_JAVA }, revision: 0, dirty: false }, listeners: new Set() };
    drafts.set(instanceId, entry);
  }
  return entry;
}

function publish(entry: DraftEntry, snapshot: JavaRuleDraft): void {
  entry.snapshot = snapshot;
  for (const listener of entry.listeners) listener();
}

export function getJavaRuleDraft(instanceId: string): JavaRuleDraft {
  return entryFor(instanceId).snapshot;
}

export function subscribeJavaRuleDraft(instanceId: string, listener: () => void): () => void {
  const entry = entryFor(instanceId);
  entry.listeners.add(listener);
  return () => { entry.listeners.delete(listener); };
}

/** Parent confirmations may update a clean card, but must never replace a newer local edit. */
export function syncConfirmedJavaRule(instanceId: string, setting: InstanceJavaSetting): void {
  const entry = entryFor(instanceId);
  const rule = javaRuleForPersistence(setting);
  if (entry.snapshot.dirty || ruleKey(entry.snapshot.confirmed) === ruleKey(rule)) return;
  publish(entry, { rule, confirmed: rule, revision: entry.snapshot.revision + 1, dirty: false });
}

function incompleteRule(rule: InstanceJavaSetting): Error | undefined {
  if (rule.mode === 'range') {
    const parsed = parseFriendlyRange(rule.range);
    if ('error' in parsed) return new LocalizedError(typeof parsed.error === 'string' ? loc('java.rangeBrackets') : parsed.error);
  }
  if (rule.mode === 'folder' && !rule.folder.trim()) return new LocalizedError(loc('java.noFolder'));
  if (rule.mode === 'forced' && !rule.path.trim()) return new LocalizedError(loc('java.noForced'));
  return undefined;
}

/** Adapt structured validation errors to existing Error|string notice handlers at display time. */
export function localizeJavaPreferenceError(error: unknown, language: Language): unknown {
  const text = localizedTextOf(error);
  return text ? renderLocalized(language, text) : error;
}

/** Staging is synchronous so a launch in the same event turn can flush the latest edit. */
export function stageJavaRuleDraft(instanceId: string, setting: InstanceJavaSetting): void {
  const entry = entryFor(instanceId);
  if (entry.timer !== undefined) clearTimeout(entry.timer);
  entry.timer = undefined;
  entry.failure = undefined;
  const rule = javaRuleForPersistence(setting);
  publish(entry, { ...entry.snapshot, rule, revision: entry.snapshot.revision + 1, dirty: true, error: undefined });
  if (!incompleteRule(rule)) {
    entry.timer = setTimeout(() => {
      entry.timer = undefined;
      void flushPendingJavaPreferences(instanceId).catch(() => undefined);
    }, 350);
  }
}

function revisions(): Map<string, number> {
  return new Map([...drafts].map(([id, entry]) => [id, entry.snapshot.revision]));
}

function acceptPreferences(preferences: JavaPreferences, expected: Map<string, number>, committedId?: string): void {
  const rules = { ...preferences.instanceRules };
  const ids = new Set([...drafts.keys(), ...Object.keys(rules)]);
  for (const id of ids) {
    const entry = entryFor(id);
    const rule = javaRuleForPersistence(rules[id] ?? DEFAULT_INSTANCE_JAVA);
    if (entry.snapshot.revision === (expected.get(id) ?? 0) && (!entry.snapshot.dirty || id === committedId)) {
      entry.failure = undefined;
      if (entry.timer !== undefined) clearTimeout(entry.timer);
      entry.timer = undefined;
      publish(entry, { rule, confirmed: rule, revision: entry.snapshot.revision + 1, dirty: false });
    }
    if (id in rules || entry.snapshot.revision > 0) rules[id] = entry.snapshot.confirmed;
  }
  cacheJavaPreferences(rules);
}

async function ensureJavaMigration(): Promise<void> {
  if (!isDesktopShell()) throw new NativeUnavailableError('get_java_preferences');
  if (!migration) {
    migration = (async () => {
      const current = await invoke<JavaPreferences>('get_java_preferences');
      if (!current.legacyMigrated) {
        /*
         * Corrupt browser data must not brick the desktop: a one-off import that keeps
         * throwing would block every Java read and launch forever. Import nothing and
         * let the marker finish; existing database values are untouched and no rule is
         * fabricated from unreadable input.
         */
        let legacy: JavaPreferences;
        try {
          legacy = readLegacyJavaPreferences();
        } catch {
          legacy = { orderedPaths: [], instanceRules: {}, legacyMigrated: false };
        }
        /*
         * Narrow the import instead of aborting it. The backend validates the whole
         * batch, so one bad rule or path would otherwise make this one-off import
         * fail on every launch. Each retry keeps strictly less data, and the last
         * attempt is empty, so a remaining failure is a real storage error and is
         * surfaced instead of being swallowed.
         */
        const candidates: JavaPreferences[] = [
          legacy,
          { ...legacy, instanceRules: {} },
          { orderedPaths: [], instanceRules: {}, legacyMigrated: false },
        ];
        for (const [index, candidate] of candidates.entries()) {
          try {
            await invoke<JavaPreferences>('migrate_legacy_java_preferences', { legacy: candidate });
            break;
          } catch (error) {
            if (index === candidates.length - 1) throw error;
          }
        }
      }
      // The database marker, not removal of browser keys, makes the import idempotent.
      try {
        localStorage.removeItem(JAVA_LIST_KEY);
        localStorage.removeItem(INSTANCE_JAVA_KEY);
      } catch { /* A blocked browser store does not undo a committed database transaction. */ }
    })().catch(error => { migration = undefined; throw error; });
  }
  await migration;
}

async function writePreferences(update: JavaPreferencesUpdate, settings?: LauncherSettings): Promise<JavaPreferences> {
  await ensureJavaMigration();
  return invoke<JavaPreferences>('save_java_preferences', {
    update: { ...update, ...(update.rule ? { rule: javaRuleForPersistence(update.rule) } : {}) },
    settings: settings ?? null,
  });
}

function trackPending(entry: DraftEntry, revision: number, promise: Promise<void>): void {
  entry.pending = { revision, promise };
  void promise.finally(() => {
    if (entry.pending?.promise === promise) entry.pending = undefined;
  }).catch(() => undefined);
}

function queueValidDrafts(excludedId?: string): void {
  for (const [id, entry] of drafts) {
    if (id !== excludedId && entry.snapshot.dirty && !entry.failure && !incompleteRule(entry.snapshot.rule)) {
      void persistDraft(id, entry).catch(() => undefined);
    }
  }
}

function persistDraft(instanceId: string, entry: DraftEntry): Promise<void> {
  const { revision, rule } = entry.snapshot;
  if (entry.timer !== undefined) clearTimeout(entry.timer);
  entry.timer = undefined;
  if (entry.pending?.revision === revision) return entry.pending.promise;
  const expected = revisions();
  const promise = serialized(async () => {
    if (entry.snapshot.revision !== revision || !entry.snapshot.dirty) return;
    try {
      const confirmed = await writePreferences({ instanceId, rule });
      acceptPreferences(confirmed, expected, instanceId);
    } catch (error) {
      if (entry.snapshot.revision === revision) {
        entry.failure = error instanceof Error ? error : new Error(String(error));
        publish(entry, { ...entry.snapshot, error: entry.failure.message });
      }
      throw error;
    }
  });
  trackPending(entry, revision, promise);
  return promise;
}

/** Flush debounce and in-flight writes. A failed/invalid current draft rejects until a new edit replaces it. */
export async function flushPendingJavaPreferences(instanceId?: string): Promise<void> {
  if (!isDesktopShell()) return;
  const targeted = () => [...drafts].filter(([id]) => instanceId === undefined || id === instanceId);
  for (;;) {
    const pending = targeted().filter(([, entry]) => entry.snapshot.dirty && !entry.failure && !incompleteRule(entry.snapshot.rule));
    await Promise.all(pending.map(([id, entry]) => persistDraft(id, entry).catch(() => undefined)));
    const barrier = operations;
    await barrier;
    if (barrier !== operations) continue;
    const dirty = targeted().filter(([, entry]) => entry.snapshot.dirty);
    for (const [, entry] of dirty) {
      const error = entry.failure ?? incompleteRule(entry.snapshot.rule);
      if (error) throw error;
    }
    if (!dirty.length) return;
  }
}

export async function loadJavaPreferences(): Promise<JavaPreferences> {
  queueValidDrafts();
  // Failed/incomplete drafts remain editable; only active valid writes precede this read.
  return serialized(async () => {
    const expected = revisions();
    await ensureJavaMigration();
    const confirmed = await invoke<JavaPreferences>('get_java_preferences');
    acceptPreferences(confirmed, expected);
    return confirmed;
  });
}

export async function loadPersistedJavaState(): Promise<{ preferences: JavaPreferences; runtimes: JavaRuntime[] }> {
  queueValidDrafts();
  return serialized(async () => {
    const expected = revisions();
    await ensureJavaMigration();
    const preferences = await invoke<JavaPreferences>('get_java_preferences');
    const runtimes = await invoke<JavaRuntime[]>('list_preferred_java_runtimes');
    acceptPreferences(preferences, expected);
    cacheJavaPreferences(Object.fromEntries([...drafts].map(([id, entry]) => [id, entry.snapshot.confirmed])), runtimes);
    return { preferences, runtimes };
  });
}

function explicitJavaUpdate<T>(
  update: JavaPreferencesUpdate,
  operation: (expected: Map<string, number>) => Promise<T>,
): Promise<T> {
  const rule = update.rule ? javaRuleForPersistence(update.rule) : undefined;
  const invalid = rule && incompleteRule(rule);
  if (invalid) return Promise.reject(invalid);
  const target = update.instanceId && rule ? entryFor(update.instanceId) : undefined;
  const replacesDraft = Boolean(target?.snapshot.dirty);
  if (target && replacesDraft) {
    if (target.timer !== undefined) clearTimeout(target.timer);
    target.timer = undefined;
    target.failure = undefined;
    // Reserve a newer revision before awaiting anything; old queued writes become stale.
    publish(target, { ...target.snapshot, rule: rule!, revision: target.snapshot.revision + 1, error: undefined });
  }
  const revision = target?.snapshot.revision;
  queueValidDrafts(target ? update.instanceId : undefined);
  const result = serialized(async () => {
    const expected = revisions();
    if (target && revision !== undefined) expected.set(update.instanceId!, revision);
    try {
      return await operation(expected);
    } catch (error) {
      if (target && replacesDraft && target.snapshot.revision === revision) {
        target.failure = error instanceof Error ? error : new Error(String(error));
        publish(target, { ...target.snapshot, error: target.failure.message });
      }
      throw error;
    }
  });
  if (target && replacesDraft && revision !== undefined) trackPending(target, revision, result.then(() => undefined));
  return result;
}

export async function saveJavaPreferences(update: JavaPreferencesUpdate, settings?: LauncherSettings): Promise<JavaPreferences> {
  return explicitJavaUpdate(update, async expected => {
    const confirmed = await writePreferences(update, settings);
    acceptPreferences(confirmed, expected, update.instanceId);
    return confirmed;
  });
}

/** A single SQLite transaction covers the settings form, Java priority and optional instance rule. */
export async function saveJavaSettingsDraft(prefs: LauncherPrefs, update: JavaPreferencesUpdate): Promise<{
  settings: LauncherSettings; prefs: LauncherPrefs; preferences: JavaPreferences;
}> {
  return explicitJavaUpdate(update, async expected => {
    const settings = prefsToSettings(prefs, await getSettings());
    const preferences = await writePreferences(update, settings);
    acceptPreferences(preferences, expected, update.instanceId);
    return { settings, preferences, prefs: prefsFromSettings(settings, prefs) };
  });
}

/** Use only for a deliberate one-launch override, never to express inheritance. */
export function launchJavaSelection(setting: InstanceJavaSetting, runtimes: JavaRuntime[], globalPath = ''): JavaSelectionRequest {
  return javaSelectionRequest(setting, runtimes, globalPath);
}
