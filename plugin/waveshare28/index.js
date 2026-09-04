'use strict';

const libQ = require('kew');
const fs = require('fs-extra');
const { execFileSync, exec } = require('child_process');
const path = require('path');
const REBOOT_SECONDS = 15;

const TOOL_PATHS = [
  '/usr/local/bin/waveshare28-config',
  '/usr/bin/waveshare28-config'
];
const SETTINGS_BACKUP_DIR = '/data/INTERNAL/waveshare28/backups';
const SETTINGS_BACKUP_SCHEMA = 1;
const SETTINGS_BACKUP_NAME_RE = /^[A-Za-z0-9._ -]{1,64}$/;
const SETTINGS_BACKUP_KEYS = ['rotation', 'speed', 'backend', 'console', 'hdmi'];
const PLUGIN_VERSION = require('./package.json').version;

module.exports = Waveshare28;

function Waveshare28(context) {
  const self = this;
  self.context = context;
  self.commandRouter = self.context.coreCommand;
  self.logger = self.context.logger;
  self.configManager = self.context.configManager;
  self.toolPath = null;
  self.board = null;
  self.rebootTimer = null;
  self.rebootLeft = 0;
}

Waveshare28.prototype.onVolumioStart = function () {
  const self = this;
  const configFile = self.commandRouter.pluginManager.getConfigurationFile(self.context, 'config.json');
  self.config = new (require('v-conf'))();
  self.config.loadFile(configFile);
  return libQ.resolve();
};

Waveshare28.prototype.getConfigurationFiles = function () {
  return ['config.json'];
};

Waveshare28.prototype.findTool = function () {
  for (let i = 0; i < TOOL_PATHS.length; i++) {
    if (fs.existsSync(TOOL_PATHS[i])) {
      return TOOL_PATHS[i];
    }
  }
  return null;
};

// Peppy-style: if the tool is not on PATH, install it from this plugin's
// payload. Do not refuse start just because /usr/local/bin is empty.
Waveshare28.prototype.ensureTool = function () {
  const self = this;
  let tool = self.findTool();
  if (tool) {
    return tool;
  }
  const installer = path.join(__dirname, 'install.sh');
  if (!fs.existsSync(installer)) {
    throw new Error('plugin payload is missing install.sh');
  }
  self.logger.info('[waveshare28] tool not on PATH; installing from plugin payload');
  execFileSync('/bin/sh', [installer], { encoding: 'utf8', timeout: 180000 });
  tool = self.findTool();
  if (!tool) {
    throw new Error('payload install finished but waveshare28-config is still missing');
  }
  return tool;
};

Waveshare28.prototype.runTool = function (args, opts) {
  const self = this;
  const tool = self.toolPath || self.findTool();
  if (!tool) {
    throw new Error('waveshare28-config is not installed');
  }
  const argv = Array.isArray(args) ? args : args.split(' ');
  try {
    // Always sudo. Volumio runs plugin code as volumio; the sudoers file
    // is the whole binary. A first unprivileged attempt prints
    // "ERROR: run as root" into the journal even when the retry works.
    return execFileSync('sudo', ['-n', tool].concat(argv), {
      encoding: 'utf8',
      timeout: 60000,
      stdio: ['ignore', 'pipe', 'pipe']
    }).trim();
  } catch (e) {
    if (!(opts && opts.quiet)) {
      self.logger.error('[waveshare28] ' + tool + ' ' + argv.join(' ') + ' failed: ' + e);
    }
    throw e;
  }
};

function fieldValue(data, key) {
  if (data[key] === undefined) {
    return undefined;
  }
  if (data[key] !== null && typeof data[key] === 'object' && data[key].value !== undefined) {
    return data[key].value;
  }
  return data[key];
}

function setSelect(item, value) {
  const match = (item.options || []).find(function (o) {
    return String(o.value) === String(value);
  });
  item.value = match || { value: value, label: String(value) };
}

function setField(section, id, fn) {
  const item = section.content.find(function (c) {
    return c.id === id;
  });
  if (item) {
    fn(item);
  }
}

function removeFields(section, ids) {
  const drop = {};
  ids.forEach(function (id) {
    drop[id] = true;
  });
  section.content = section.content.filter(function (item) {
    return !drop[item.id];
  });
  if (section.saveButton && section.saveButton.data) {
    section.saveButton.data = section.saveButton.data.filter(function (id) {
      return !drop[id];
    });
  }
}

Waveshare28.prototype.onStart = function () {
  const self = this;
  const defer = libQ.defer();

  try {
    self.toolPath = self.ensureTool();
    self.board = JSON.parse(self.runTool(['detect']));
  } catch (e) {
    self.logger.error('[waveshare28] start failed: ' + e);
    self.commandRouter.pushToastMessage('error', 'Waveshare 2.8', String(e.message || e));
    defer.reject(e);
    return defer.promise;
  }

  if (!self.board.supported) {
    const why = self.board.reason === 'armv6'
      ? 'This board is armv6 (Pi 1 / original Pi Zero) and is not supported.'
      : 'This plugin requires a Raspberry Pi (armv7 or later).';
    self.logger.error('[waveshare28] unsupported board: ' + self.board.reason);
    self.commandRouter.pushToastMessage('error', 'Waveshare 2.8', why);
    defer.reject(new Error('Unsupported board: ' + self.board.reason));
    return defer.promise;
  }

  self.logger.info('[waveshare28] using ' + self.toolPath + ' on ' + self.board.family);
  defer.resolve();
  return defer.promise;
};

Waveshare28.prototype.onStop = function () {
  const self = this;
  const defer = libQ.defer();
  self.clearRebootTimer();
  try {
    if (self.findTool()) {
      self.runTool(['recover']);
    }
  } catch (e) {
    self.logger.error('[waveshare28] recover on stop failed: ' + e);
  }
  self.toolPath = null;
  self.board = null;
  defer.resolve();
  return defer.promise;
};

Waveshare28.prototype.getUIConfig = function () {
  const self = this;
  const defer = libQ.defer();
  const lang_code = self.commandRouter.sharedVars.get('language_code');

  self.commandRouter.i18nJson(
    path.join(__dirname, 'i18n', 'strings_' + lang_code + '.json'),
    path.join(__dirname, 'i18n', 'strings_en.json'),
    path.join(__dirname, 'UIConfig.json')
  )
    .then(function (uiconf) {
      let state;
      try {
        if (!self.findTool()) {
          self.ensureTool();
        }
        state = JSON.parse(self.runTool(['show', '--json']));
      } catch (e) {
        self.logger.error('[waveshare28] show --json failed: ' + e);
        defer.reject(e);
        return;
      }

      const board = state.board || self.board || {};
      const params = board.params || {};
      const status = uiconf.sections[0];
      const settings = uiconf.sections[1];

      setField(status, 'board_family', function (item) {
        item.value = (board.family || '') + (board.revision ? ' (' + board.revision + ')' : '');
      });
      setField(status, 'board_model', function (item) {
        item.value = board.model || '';
      });
      setField(status, 'kms3a', function (item) {
        item.value = board.kms3a || 'n/a';
      });
      if (board.family !== 'pi3a+') {
        removeFields(status, ['kms3a']);
      }

      setField(settings, 'rotation', function (item) {
        setSelect(item, state.rotation);
      });
      setField(settings, 'speed', function (item) {
        item.value = String(state.speed);
      });
      setField(settings, 'backend', function (item) {
        setSelect(item, state.backend);
      });
      setField(settings, 'console', function (item) {
        setSelect(item, state.console);
      });
      setField(settings, 'hdmi', function (item) {
        item.value = state.hdmi === 'on';
      });

      if (!params.hdmi) {
        removeFields(settings, ['hdmi']);
      }

      const backups = self.listSettingsBackups();
      const none = { value: '', label: 'No settings backups yet' };
      uiconf.sections.forEach(function (section) {
        if (!section.content) {
          return;
        }
        section.content.forEach(function (el) {
          if (el.id !== 'selected_backup' && el.id !== 'selected_backup_delete') {
            return;
          }
          el.options = backups.length ? backups : [none];
          el.value = backups.length
            ? { value: backups[0].value, label: backups[0].label }
            : { value: none.value, label: none.label };
        });
      });

      defer.resolve(uiconf);
      if (self.rebootLeft > 0) {
        self.showRebootModal(self.rebootLeft);
      }
    })
    .fail(function (error) {
      self.logger.error('[waveshare28] Failed to load UI config: ' + error);
      defer.reject(error);
    });

  return defer.promise;
};

Waveshare28.prototype.saveSettings = function (data) {
  const self = this;
  const defer = libQ.defer();
  try {
    const before = JSON.parse(self.runTool(['show', '--json']));
    const args = [];
    const rotation = fieldValue(data, 'rotation');
    const speed = fieldValue(data, 'speed');
    const backend = fieldValue(data, 'backend');
    const consoleMode = fieldValue(data, 'console');
    if (rotation !== undefined) {
      args.push('rotation=' + rotation);
    }
    if (speed !== undefined) {
      args.push('speed=' + String(speed).trim());
    }
    if (backend !== undefined) {
      args.push('backend=' + backend);
    }
    if (consoleMode !== undefined && backend === 'framebuffer') {
      args.push('console=' + consoleMode);
    }
    if (data.hdmi !== undefined) {
      args.push('hdmi=' + (data.hdmi ? 'on' : 'off'));
    }
    if (args.length === 0) {
      defer.resolve();
      return defer.promise;
    }
    self.runTool(['set'].concat(args));
    const after = JSON.parse(self.runTool(['show', '--json']));
    self.afterSetMaybeReboot(before, after, 'Settings applied.');
    defer.resolve();
  } catch (e) {
    self.logger.error('[waveshare28] set failed: ' + e);
    self.commandRouter.pushToastMessage('error', 'Waveshare 2.8', 'Failed to apply settings.');
    defer.reject(e);
  }
  return defer.promise;
};

Waveshare28.prototype.coreI18n = function (key, fallback) {
  try {
    const s = this.commandRouter.getI18nString && this.commandRouter.getI18nString(key);
    if (s && s !== key) {
      return s;
    }
  } catch (e) {
    /* core string missing */
  }
  return fallback;
};

// Firmware overlays (backend, fbtft rotate/speed, HDMI) are not live
// until the next boot. console= only rewrites the unit.
Waveshare28.prototype.afterSetMaybeReboot = function (before, after, okToast) {
  if (this.needsReboot(before, after)) {
    this.logger.info(
      '[waveshare28] firmware overlay changed (' +
        (before && before.backend) +
        ' -> ' +
        (after && after.backend) +
        '); reboot required'
    );
    this.initRebootCountdown();
    return;
  }
  this.commandRouter.pushToastMessage('success', 'Waveshare 2.8', okToast);
};

Waveshare28.prototype.needsReboot = function (before, after) {
  if (!before || !after) {
    return true;
  }
  if (before.backend !== after.backend) {
    return true;
  }
  if (after.backend === 'framebuffer' || before.backend === 'framebuffer') {
    if (Number(before.rotation) !== Number(after.rotation)) {
      return true;
    }
    if (Number(before.speed) !== Number(after.speed)) {
      return true;
    }
    if (before.hdmi !== after.hdmi) {
      return true;
    }
  }
  return false;
};

Waveshare28.prototype.showRebootModal = function (seconds) {
  this.commandRouter.broadcastMessage('openModal', {
    title: 'Waveshare 2.8',
    message: 'Settings saved. A reboot is required for the firmware overlay. Your device will restart in ' + seconds + ' seconds.',
    size: 'lg',
    buttons: [
      {
        name: this.coreI18n('COMMON.RESTART', 'Restart'),
        class: 'btn btn-info',
        emit: 'callMethod',
        payload: {
          endpoint: 'system_controller/waveshare28',
          method: 'finishReboot',
          data: {}
        }
      },
      {
        name: this.coreI18n('COMMON.CANCEL', 'Cancel'),
        class: 'btn btn-warning',
        emit: 'callMethod',
        payload: {
          endpoint: 'system_controller/waveshare28',
          method: 'cancelReboot',
          data: {}
        }
      }
    ]
  });
};

Waveshare28.prototype.clearRebootTimer = function () {
  if (this.rebootTimer) {
    clearInterval(this.rebootTimer);
    this.rebootTimer = null;
  }
};

Waveshare28.prototype.finishReboot = function () {
  this.rebootLeft = 0;
  this.clearRebootTimer();
  if (typeof this.commandRouter.closeModals === 'function') {
    this.commandRouter.closeModals();
  }
  this.logger.info('[waveshare28] rebooting after firmware overlay change');
  if (typeof this.commandRouter.reboot === 'function') {
    this.commandRouter.reboot();
  } else {
    exec('/usr/bin/sudo /sbin/reboot', { timeout: 15000 }, function () {});
  }
};

Waveshare28.prototype.initRebootCountdown = function () {
  const self = this;
  self.clearRebootTimer();
  self.rebootLeft = REBOOT_SECONDS;
  // A section onSave refreshes the plugin page and closes a modal
  // opened in the same tick. Wait for that redraw.
  setTimeout(function () {
    if (self.rebootLeft <= 0) {
      return;
    }
    self.showRebootModal(self.rebootLeft);
    self.rebootTimer = setInterval(function () {
      self.rebootLeft -= 1;
      if (self.rebootLeft > 0) {
        self.showRebootModal(self.rebootLeft);
      } else {
        self.finishReboot();
      }
    }, 1000);
  }, 500);
};

Waveshare28.prototype.cancelReboot = function () {
  this.rebootLeft = 0;
  this.clearRebootTimer();
  if (typeof this.commandRouter.closeModals === 'function') {
    this.commandRouter.closeModals();
  }
  this.commandRouter.pushToastMessage(
    'info',
    'Waveshare 2.8',
    'Reboot cancelled. Settings are already saved; reboot when you can.'
  );
  return this.updateUIConfig();
};

Waveshare28.prototype.updateUIConfig = function () {
  const self = this;
  if (typeof this.commandRouter.getUIConfigOnPlugin !== 'function') {
    return libQ.resolve();
  }
  return this.commandRouter
    .getUIConfigOnPlugin('system_controller', 'waveshare28', {})
    .then(function (uiconf) {
      self.commandRouter.broadcastMessage('pushUiConfig', uiconf);
    })
    .fail(function (e) {
      self.logger.error('[waveshare28] pushUiConfig failed: ' + e);
      return libQ.resolve();
    });
};

Waveshare28.prototype.settingsBackupDir = function () {
  return this._settingsBackupDir || SETTINGS_BACKUP_DIR;
};

Waveshare28.prototype.sanitizeBackupName = function (raw) {
  const name = String(raw == null ? '' : raw).trim();
  if (!name || name.indexOf('..') !== -1 || /[\\/]/.test(name)) {
    return '';
  }
  if (!SETTINGS_BACKUP_NAME_RE.test(name)) {
    return '';
  }
  return name;
};

Waveshare28.prototype.settingsBackupPath = function (name) {
  return path.join(this.settingsBackupDir(), name + '.json');
};

Waveshare28.prototype.ensureSettingsBackupDir = function () {
  const dir = this.settingsBackupDir();
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  try {
    fs.chmodSync(dir, 0o700);
  } catch (e) {
    /* some filesystems refuse mode */
  }
  return dir;
};

Waveshare28.prototype.settingsBackupSnapshot = function () {
  const state = JSON.parse(this.runTool(['show', '--json']));
  const values = {};
  SETTINGS_BACKUP_KEYS.forEach(function (key) {
    values[key] = state[key];
  });
  return {
    schema_version: SETTINGS_BACKUP_SCHEMA,
    plugin_version: PLUGIN_VERSION,
    created: new Date().toISOString(),
    values: values
  };
};

Waveshare28.prototype.validateBackupValues = function (values) {
  if (!values || typeof values !== 'object') {
    return { ok: false, message: 'That settings backup has no values.' };
  }
  const rotation = parseInt(values.rotation, 10);
  if ([0, 90, 180, 270].indexOf(rotation) === -1) {
    return { ok: false, message: 'That settings backup has an invalid rotation.' };
  }
  const speed = parseInt(values.speed, 10);
  if (!Number.isFinite(speed) || speed <= 0) {
    return { ok: false, message: 'That settings backup has an invalid speed.' };
  }
  if (values.backend !== 'spi' && values.backend !== 'framebuffer') {
    return { ok: false, message: 'That settings backup has an invalid backend.' };
  }
  if (values.console !== 'share' && values.console !== 'release') {
    return { ok: false, message: 'That settings backup has an invalid console.' };
  }
  if (values.hdmi !== 'on' && values.hdmi !== 'off') {
    return { ok: false, message: 'That settings backup has an invalid hdmi.' };
  }
  return {
    ok: true,
    values: {
      rotation: rotation,
      speed: speed,
      backend: values.backend,
      console: values.console,
      hdmi: values.hdmi
    }
  };
};

Waveshare28.prototype.readSettingsBackup = function (name) {
  const safe = this.sanitizeBackupName(name);
  if (!safe) {
    return { ok: false, message: 'Choose a settings backup.' };
  }
  let raw;
  try {
    raw = fs.readFileSync(this.settingsBackupPath(safe), 'utf8');
  } catch (e) {
    return { ok: false, message: 'That settings backup is not on this device.' };
  }
  let snap;
  try {
    snap = JSON.parse(raw);
  } catch (e) {
    return { ok: false, message: 'That settings backup is not valid JSON.' };
  }
  if (!snap || snap.schema_version !== SETTINGS_BACKUP_SCHEMA ||
      !snap.values || typeof snap.values !== 'object') {
    return { ok: false, message: 'That settings backup is not a schema 1 snapshot.' };
  }
  return { ok: true, name: safe, snapshot: snap };
};

Waveshare28.prototype.listSettingsBackups = function () {
  let names;
  try {
    names = fs.readdirSync(this.settingsBackupDir());
  } catch (e) {
    return [];
  }
  const options = [];
  for (let i = 0; i < names.length; i++) {
    const file = names[i];
    if (!file.endsWith('.json')) {
      continue;
    }
    const name = file.slice(0, -5);
    if (!this.sanitizeBackupName(name)) {
      continue;
    }
    const read = this.readSettingsBackup(name);
    if (!read.ok) {
      continue;
    }
    options.push({ value: name, label: name });
  }
  options.sort(function (a, b) {
    return a.value.localeCompare(b.value);
  });
  return options;
};

Waveshare28.prototype.createSettingsBackup = function (data) {
  const name = this.sanitizeBackupName(data && data.backup_name);
  if (!name) {
    this.commandRouter.pushToastMessage(
      'error',
      'Waveshare 2.8',
      'Backup name must be 1–64 letters, numbers, spaces, dots, underscores or hyphens.'
    );
    return libQ.resolve();
  }
  try {
    this.ensureSettingsBackupDir();
    const snap = this.settingsBackupSnapshot();
    snap.name = name;
    fs.writeFileSync(this.settingsBackupPath(name), JSON.stringify(snap, null, 2) + '\n', { mode: 0o640 });
  } catch (e) {
    this.logger.error('[waveshare28] settings backup failed: ' + e);
    this.commandRouter.pushToastMessage('error', 'Waveshare 2.8', 'Could not write the settings backup.');
    return libQ.resolve();
  }
  this.commandRouter.pushToastMessage('success', 'Waveshare 2.8', 'Settings backup saved.');
  return this.updateUIConfig();
};

Waveshare28.prototype.restoreSettingsBackup = function (data) {
  const name = fieldValue(data || {}, 'selected_backup');
  const read = this.readSettingsBackup(name);
  if (!read.ok) {
    this.commandRouter.pushToastMessage('error', 'Waveshare 2.8', read.message);
    return libQ.resolve();
  }
  const checked = this.validateBackupValues(read.snapshot.values);
  if (!checked.ok) {
    this.logger.error('[waveshare28] rejected settings backup: ' + checked.message);
    this.commandRouter.pushToastMessage('error', 'Waveshare 2.8', checked.message);
    return libQ.resolve();
  }
  const before = JSON.parse(this.runTool(['show', '--json']));
  try {
    const v = checked.values;
    const args = [
      'rotation=' + v.rotation,
      'speed=' + v.speed,
      'backend=' + v.backend
    ];
    if (v.backend === 'framebuffer') {
      args.push('console=' + v.console);
      if (this.board && this.board.params && this.board.params.hdmi) {
        args.push('hdmi=' + v.hdmi);
      }
    }
    this.runTool(['set'].concat(args));
  } catch (e) {
    this.logger.error('[waveshare28] restore set failed: ' + e);
    this.commandRouter.pushToastMessage('error', 'Waveshare 2.8', 'Failed to restore settings.');
    return libQ.resolve();
  }
  const after = JSON.parse(this.runTool(['show', '--json']));
  this.afterSetMaybeReboot(before, after, 'Settings restored.');
  return this.updateUIConfig();
};

Waveshare28.prototype.deleteSettingsBackup = function (data) {
  const name = this.sanitizeBackupName(fieldValue(data || {}, 'selected_backup_delete'));
  if (!name) {
    this.commandRouter.pushToastMessage('error', 'Waveshare 2.8', 'Choose a settings backup.');
    return libQ.resolve();
  }
  try {
    fs.unlinkSync(this.settingsBackupPath(name));
  } catch (e) {
    this.commandRouter.pushToastMessage('error', 'Waveshare 2.8', 'That settings backup is not on this device.');
    return libQ.resolve();
  }
  this.commandRouter.pushToastMessage('success', 'Waveshare 2.8', 'Settings backup deleted.');
  return this.updateUIConfig();
};

Waveshare28.prototype.runVerify = function () {
  const self = this;
  const defer = libQ.defer();
  try {
    const out = self.runTool(['verify'], { quiet: true });
    self.commandRouter.pushToastMessage('success', 'Waveshare 2.8', 'No drift.');
    self.logger.info('[waveshare28] verify:\n' + out);
    defer.resolve();
  } catch (e) {
    const out = (e.stdout || e.message || String(e)).toString();
    self.commandRouter.pushToastMessage('warning', 'Waveshare 2.8', 'Configuration has drifted. Apply settings to restore.');
    self.logger.warn('[waveshare28] verify:\n' + out);
    defer.resolve();
  }
  return defer.promise;
};
