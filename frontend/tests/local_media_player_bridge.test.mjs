import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import test from 'node:test';

const bridgePath = path.resolve('frontend/static/local_media_player_bridge.js');
const bridgeSource = fs.readFileSync(bridgePath, 'utf8');

function createEnvironment({ nativeHls = false } = {}) {
  const mounts = [];
  const storage = new Map();

  function Player(config) {
    this.config = config;
    this.destroy = () => {};
    this.on = () => {};
    this.once = () => {};
    mounts.push({ ctor: 'Player', config, instance: this });
  }
  Player.defaultPreset = { name: 'default-preset' };

  function HlsPlayerPlugin() {}
  HlsPlayerPlugin.pluginName = 'hls';
  HlsPlayerPlugin.isSupported = () => true;

  const window = {
    Player,
    HlsPlayer: HlsPlayerPlugin,
    innerWidth: 390,
    matchMedia: () => ({ matches: true }),
    localStorage: {
      getItem(key) {
        return storage.has(key) ? storage.get(key) : null;
      },
      setItem(key, value) {
        storage.set(key, String(value));
      },
      removeItem(key) {
        storage.delete(key);
      },
    },
    setTimeout(fn) {
      fn();
      return 1;
    },
  };

  const document = {
    createElement(tag) {
      if (tag !== 'video') {
        throw new Error(`unexpected tag: ${tag}`);
      }
      return {
        canPlayType(mime) {
          return nativeHls && mime === 'application/vnd.apple.mpegurl' ? 'probably' : '';
        },
      };
    },
  };

  const context = vm.createContext({
    window,
    document,
    console,
    Number,
    Date,
  });
  vm.runInContext(bridgeSource, context, { filename: bridgePath });

  const element = {
    __sfLocalMediaPlayer: null,
    innerHTML: '',
  };

  return { window, mounts, element };
}

test('hls mode uses Player with HlsPlayer plugin instead of constructing the plugin directly', () => {
  const { window, mounts, element } = createEnvironment({ nativeHls: false });

  window.sfLocalMediaPlayerMount(
    element,
    '/admin/local-media/api/playback/hls/demo/index.m3u8',
    'hls',
    'Demo',
    'sf-local-media-progress:demo',
  );

  assert.equal(mounts.length, 1);
  assert.equal(mounts[0].ctor, 'Player');
  assert.equal(mounts[0].config.plugins.length, 1);
  assert.equal(mounts[0].config.plugins[0], window.HlsPlayer);
  assert.equal(mounts[0].config.url, '/admin/local-media/api/playback/hls/demo/index.m3u8');
  assert.equal(mounts[0].config.isLive, false);
});

test('player bridge uses Player.defaultPreset when available', () => {
  const { mounts, element, window } = createEnvironment({ nativeHls: false });

  window.sfLocalMediaPlayerMount(
    element,
    '/admin/local-media/api/playback/raw?file=demo.mp4',
    'raw',
    'Demo',
    'sf-local-media-progress:demo',
  );

  assert.equal(mounts.length, 1);
  assert.equal(mounts[0].config.presets.length, 1);
  assert.equal(mounts[0].config.presets[0], window.Player.defaultPreset);
});
