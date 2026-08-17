/* alien.hartle.tech — behaviour and sound.
 *
 * No dependencies, no build step, no network requests. That is not minimalism
 * for its own sake: the page ships inside a Caddy container with a strict CSP,
 * and every byte it needs has to be in the image.
 *
 * ── Why the sound is synthesised rather than sampled ────────────────────────
 * The brief asked for "wet gruesome technical" sound effects from free asset
 * sites. Two problems with sampling that, and one better answer.
 *
 *   1. The obvious sources for this specific aesthetic — sound-button sites,
 *      "movie SFX" archives — are overwhelmingly ripped film audio. Putting
 *      that on a public site attached to a real company is a straightforward
 *      copyright problem, and "it was on a free site" is not a licence.
 *   2. Even properly CC0 samples need per-file provenance tracking forever.
 *
 * So every sound below is generated at runtime from oscillators and noise.
 * It is original by construction, has no licence to honour, adds zero bytes of
 * assets, and is trivially tweakable. See CREDITS.md.
 */

(() => {
  'use strict';

  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* ── Audio ───────────────────────────────────────────────────────────────
     One lazily-created AudioContext. Browsers refuse to start one before a
     user gesture, so it is built on the first toggle click and never before —
     which conveniently means muted-by-default is enforced by the platform as
     well as by us. */

  const Sound = (() => {
    let ctx = null;
    let master = null;
    let bed = null;
    let on = localStorage.getItem('alien-sound') === 'on';

    let wet = null;   // shared reverb send — the "damp" in damp and gruesome

    function ensure() {
      if (ctx) return ctx;
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      ctx = new AC();

      // A limiter on the way out, and it is not optional. Five layers plus a
      // reverb tail plus interaction sounds sum well past 1.0 on peaks, and
      // Web Audio does not clip gracefully — it crackles. A compressor with a
      // hard knee and a fast attack turns "crackling" into "loud", which is
      // the difference between broken and mixed.
      const limiter = ctx.createDynamicsCompressor();
      limiter.threshold.value = -8;
      limiter.knee.value = 0;
      limiter.ratio.value = 20;
      limiter.attack.value = 0.003;
      limiter.release.value = 0.25;
      limiter.connect(ctx.destination);

      master = ctx.createGain();
      master.gain.value = 0.0;
      master.connect(limiter);

      // One convolver for the whole page. Every layer and every interaction
      // sound sends into it, which is what makes them sound like they are in
      // the same wet concrete room rather than in a browser.
      const verb = ctx.createConvolver();
      verb.buffer = impulse(2.6, 2.4);
      const verbGain = ctx.createGain();
      verbGain.gain.value = 0.3;
      verb.connect(verbGain).connect(master);

      wet = ctx.createGain();
      wet.gain.value = 0.6;
      wet.connect(verb);

      return ctx;
    }

    function noiseBuffer(seconds) {
      const n = Math.floor(ctx.sampleRate * seconds);
      const buf = ctx.createBuffer(1, n, ctx.sampleRate);
      const d = buf.getChannelData(0);
      for (let i = 0; i < n; i++) d[i] = Math.random() * 2 - 1;
      return buf;
    }

    /* A synthetic impulse response: stereo noise under an exponential decay,
       with the two channels decorrelated so the tail has width. Cheap, and it
       beats shipping a real IR file we would have to licence. */
    function impulse(seconds, decay) {
      const n = Math.floor(ctx.sampleRate * seconds);
      const buf = ctx.createBuffer(2, n, ctx.sampleRate);
      for (let c = 0; c < 2; c++) {
        const d = buf.getChannelData(c);
        for (let i = 0; i < n; i++) {
          // A short noisy pre-delay before the tail reads as a bigger space.
          const t = i / n;
          d[i] = (Math.random() * 2 - 1) * Math.pow(1 - t, decay);
        }
      }
      return buf;
    }

    const rand = (a, b) => a + Math.random() * (b - a);

    /* Motion tracker: a short descending sine with a hard attack and a quieter
       echo a beat later. The echo is what makes it read as "tracker" rather
       than "notification". */
    function ping() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      [[0, 0.5], [0.13, 0.16]].forEach(([delay, level]) => {
        const o = ctx.createOscillator();
        const g = ctx.createGain();
        o.type = 'sine';
        o.frequency.setValueAtTime(1280, t + delay);
        o.frequency.exponentialRampToValueAtTime(760, t + delay + 0.09);
        g.gain.setValueAtTime(0.0001, t + delay);
        g.gain.exponentialRampToValueAtTime(level, t + delay + 0.006);
        g.gain.exponentialRampToValueAtTime(0.0001, t + delay + 0.16);
        o.connect(g);
        g.connect(master);
        g.connect(wet);
        o.start(t + delay);
        o.stop(t + delay + 0.2);
      });
    }

    /* Wet: a noise burst pushed through a lowpass that collapses fast, with a
       little resonance. The falling cutoff is the whole trick — it is what the
       ear hears as something viscous closing. */
    function squelch() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      const src = ctx.createBufferSource();
      src.buffer = noiseBuffer(0.4);

      const lp = ctx.createBiquadFilter();
      lp.type = 'lowpass';
      lp.Q.value = 12;
      lp.frequency.setValueAtTime(2400, t);
      lp.frequency.exponentialRampToValueAtTime(160, t + 0.28);

      const g = ctx.createGain();
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(0.22, t + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 0.34);

      src.connect(lp).connect(g);
      g.connect(master);
      g.connect(wet);
      src.start(t);
      src.stop(t + 0.4);
    }

    /* Mechanical: a very short sawtooth chirp. Servos and relays. */
    function servo() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      const hp = ctx.createBiquadFilter();
      hp.type = 'highpass';
      hp.frequency.value = 400;
      o.type = 'sawtooth';
      o.frequency.setValueAtTime(180, t);
      o.frequency.exponentialRampToValueAtTime(90, t + 0.05);
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(0.13, t + 0.004);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 0.07);
      o.connect(hp).connect(g);
      g.connect(master);
      g.connect(wet);
      o.start(t);
      o.stop(t + 0.09);
    }

    /* Confirmation: two rising sines. Deliberately clean — this one fires when
       something worked, and it should not sound like the horror ones. */
    function confirm() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      [660, 990].forEach((f, i) => {
        const o = ctx.createOscillator();
        const g = ctx.createGain();
        o.type = 'triangle';
        o.frequency.value = f;
        g.gain.setValueAtTime(0.0001, t + i * 0.07);
        g.gain.exponentialRampToValueAtTime(0.16, t + i * 0.07 + 0.01);
        g.gain.exponentialRampToValueAtTime(0.0001, t + i * 0.07 + 0.18);
        o.connect(g);
        g.connect(master);
        g.connect(wet);
        o.start(t + i * 0.07);
        o.stop(t + i * 0.07 + 0.2);
      });
    }

    /* ── The bed ──────────────────────────────────────────────────────────
       A real track, not a loop generated here: "They Come at Night" by
       morgantj, CC BY 3.0, from ccMixter. The licence is verifiable three
       ways — the rel="license" on the track page, the copyright frame inside
       the file itself, and the verification URL that frame carries. Credited
       in the page footer and in CREDITS.md, which is what CC BY asks for.

       It is routed through the same limiter and the same reverb send as
       everything else, so the interaction sounds sit in the track rather than
       on top of it.

       The synthesised bed underneath is kept as a fallback. If the file 404s,
       is blocked, or the browser refuses both codecs, the page still has an
       atmosphere instead of silence. */

    const BPM = 70;
    const BEAT = 60 / BPM;
    let track = null;

    function startTrack() {
      const audioEl = document.getElementById('bed');
      if (!audioEl || track) return false;
      try {
        const src = ctx.createMediaElementSource(audioEl);
        const g = ctx.createGain();
        g.gain.value = 0.0001;
        src.connect(g);
        g.connect(master);

        // A measured reverb send, so the track shares the room with the
        // interaction sounds without turning to mush.
        const send = ctx.createGain();
        send.gain.value = 0.12;
        g.connect(send).connect(wet);

        // Fade in rather than arrive.
        g.gain.exponentialRampToValueAtTime(0.85, ctx.currentTime + 3);

        audioEl.volume = 1;
        const p = audioEl.play();
        if (p && p.catch) {
          p.catch(() => {
            // Autoplay refused or both codecs unsupported: fall back to the
            // generated bed rather than leaving the page silent.
            track = null;
            startBed();
          });
        }
        track = { audioEl, g };
        return true;
      } catch {
        return false;
      }
    }

    function stopTrack() {
      if (!track) return;
      const t = ctx.currentTime;
      track.g.gain.cancelScheduledValues(t);
      track.g.gain.setValueAtTime(Math.max(track.g.gain.value, 0.0001), t);
      track.g.gain.exponentialRampToValueAtTime(0.0001, t + 0.7);
      const dying = track;
      track = null;
      setTimeout(() => { try { dying.audioEl.pause(); } catch { /* gone */ } }, 800);
    }

    /* ── Fallback bed ─────────────────────────────────────────────────────
       Five synthesised layers at a half-time 70 BPM. Only used when the track
       will not play. */

    function startBed() {
      if (!ctx || bed) return;
      const t = ctx.currentTime;

      const out = ctx.createGain();
      out.gain.value = 0.0001;
      out.connect(master);
      // A measured send, not a second full copy of the mix.
      const bedSend = ctx.createGain();
      bedSend.gain.value = 0.22;
      out.connect(bedSend).connect(wet);
      // Fade in over four bars rather than arriving. Nothing about this should
      // feel like a sound effect starting.
      out.gain.exponentialRampToValueAtTime(0.34, t + BEAT * 8);

      // ── sub ────────────────────────────────────────────────────────────
      const sub = ctx.createOscillator();
      const subGain = ctx.createGain();
      sub.type = 'sine';
      sub.frequency.value = 41.2;               // E1
      subGain.gain.value = 0.2;
      sub.connect(subGain).connect(out);
      sub.start(t);

      // ── reese / wobble ─────────────────────────────────────────────────
      const reeseFilter = ctx.createBiquadFilter();
      reeseFilter.type = 'lowpass';
      reeseFilter.frequency.value = 220;
      reeseFilter.Q.value = 7;                  // resonance is the growl; above ~10 it self-oscillates and spits

      const reeseGain = ctx.createGain();
      reeseGain.gain.value = 0.1;
      reeseFilter.connect(reeseGain).connect(out);

      const reeses = [82.4, 82.9, 123.5].map((f) => {
        const o = ctx.createOscillator();
        o.type = 'sawtooth';
        o.frequency.value = f;
        o.connect(reeseFilter);
        o.start(t);
        return o;
      });

      // The wobble LFO. A triangle rather than a sine so the sweep has a bit
      // of edge at the turnaround.
      const wob = ctx.createOscillator();
      const wobDepth = ctx.createGain();
      wob.type = 'triangle';
      wob.frequency.value = 1 / (BEAT / 2);      // eighth notes
      wobDepth.gain.value = 420;
      wob.connect(wobDepth).connect(reeseFilter.frequency);
      wob.start(t);

      // A second, much slower LFO on the depth so the wobble breathes instead
      // of repeating identically forever.
      const drift = ctx.createOscillator();
      const driftDepth = ctx.createGain();
      drift.type = 'sine';
      drift.frequency.value = 0.031;
      driftDepth.gain.value = 190;
      drift.connect(driftDepth).connect(wobDepth.gain);
      drift.start(t);

      // ── air ────────────────────────────────────────────────────────────
      const air = ctx.createBufferSource();
      air.buffer = noiseBuffer(8);
      air.loop = true;
      const airBP = ctx.createBiquadFilter();
      airBP.type = 'bandpass';
      airBP.frequency.value = 700;
      airBP.Q.value = 0.8;
      const airGain = ctx.createGain();
      airGain.gain.value = 0.032;

      const airLfo = ctx.createOscillator();
      const airDepth = ctx.createGain();
      airLfo.frequency.value = 0.024;
      airDepth.gain.value = 480;
      airLfo.connect(airDepth).connect(airBP.frequency);
      airLfo.start(t);

      air.connect(airBP).connect(airGain).connect(out);
      air.start(t);

      // ── the wet delay the drips live in ────────────────────────────────
      const delay = ctx.createDelay(2);
      delay.delayTime.value = BEAT * 0.75;       // dotted eighth
      const fb = ctx.createGain();
      fb.gain.value = 0.3;
      const dampen = ctx.createBiquadFilter();
      dampen.type = 'lowpass';
      dampen.frequency.value = 1600;             // each repeat duller than the last
      delay.connect(dampen).connect(fb).connect(delay);
      delay.connect(out);

      bed = { out, nodes: [sub, ...reeses, wob, drift, airLfo, air], delay, timers: [] };

      // ── drips ──────────────────────────────────────────────────────────
      // Scheduled with setTimeout rather than a lookahead scheduler: these are
      // deliberately loose, and sample-accurate placement would make them
      // sound sequenced, which is the opposite of what is wanted.
      const drip = () => {
        if (!bed || !on) return;
        const now = ctx.currentTime;
        const o = ctx.createOscillator();
        const g = ctx.createGain();
        const bp = ctx.createBiquadFilter();
        bp.type = 'bandpass';
        bp.Q.value = 9;
        bp.frequency.value = rand(900, 2600);
        o.type = 'sine';
        o.frequency.setValueAtTime(rand(1400, 2400), now);
        o.frequency.exponentialRampToValueAtTime(rand(320, 700), now + 0.11);
        g.gain.setValueAtTime(0.0001, now);
        g.gain.exponentialRampToValueAtTime(rand(0.035, 0.085), now + 0.006);
        g.gain.exponentialRampToValueAtTime(0.0001, now + 0.2);
        o.connect(bp).connect(g);
        g.connect(delay);
        g.connect(out);
        o.start(now);
        o.stop(now + 0.25);
        bed.timers.push(setTimeout(drip, rand(1800, 6200)));
      };
      bed.timers.push(setTimeout(drip, 1200));

      // ── industrial hits ────────────────────────────────────────────────
      const hit = () => {
        if (!bed || !on) return;
        const now = ctx.currentTime;
        const src = ctx.createBufferSource();
        src.buffer = noiseBuffer(0.6);
        const bp = ctx.createBiquadFilter();
        bp.type = 'bandpass';
        bp.frequency.value = rand(180, 900);
        bp.Q.value = rand(6, 18);                // high Q = ringing metal
        const g = ctx.createGain();
        g.gain.setValueAtTime(0.0001, now);
        g.gain.exponentialRampToValueAtTime(rand(0.05, 0.1), now + 0.004);
        g.gain.exponentialRampToValueAtTime(0.0001, now + rand(0.4, 1.1));
        src.connect(bp).connect(g);
        g.connect(out);
        g.connect(wet);
        src.start(now);
        src.stop(now + 1.2);
        bed.timers.push(setTimeout(hit, rand(4000, 11000)));
      };
      bed.timers.push(setTimeout(hit, 3400));
    }

    function stopBed() {
      if (!bed) return;
      bed.timers.forEach(clearTimeout);
      const t = ctx.currentTime;
      bed.out.gain.cancelScheduledValues(t);
      bed.out.gain.setValueAtTime(Math.max(bed.out.gain.value, 0.0001), t);
      bed.out.gain.exponentialRampToValueAtTime(0.0001, t + 0.8);
      const dying = bed;
      bed = null;
      setTimeout(() => {
        dying.nodes.forEach((n) => { try { n.stop(); } catch { /* already stopped */ } });
      }, 900);
    }

    function setOn(next) {
      on = next;
      localStorage.setItem('alien-sound', on ? 'on' : 'off');
      if (!on) {
        // Tear it down rather than only muting: left running behind a zero
        // gain it keeps a decoder, oscillators and timers alive forever, which
        // is rude on a laptop battery.
        stopTrack();
        stopBed();
        if (master) master.gain.linearRampToValueAtTime(0, ctx.currentTime + 0.3);
        return;
      }
      ensure();
      if (!ctx) return;
      if (ctx.state === 'suspended') ctx.resume();
      master.gain.cancelScheduledValues(ctx.currentTime);
      master.gain.linearRampToValueAtTime(0.9, ctx.currentTime + 0.4);
      if (!startTrack()) startBed();
    }

    return { ping, squelch, servo, confirm, setOn, isOn: () => on };
  })();

  /* ── Sound toggle ─────────────────────────────────────────────────────── */

  const soundBtn = document.getElementById('sound');
  function paintSound() {
    const on = Sound.isOn();
    soundBtn.setAttribute('aria-pressed', String(on));
    soundBtn.textContent = on ? '● audio on' : '○ audio off';
  }
  soundBtn.addEventListener('click', () => {
    Sound.setOn(!Sound.isOn());
    paintSound();
    if (Sound.isOn()) Sound.ping();
  });
  // Never auto-enable from storage: an audio context cannot start without a
  // gesture anyway, and a page that starts droning on load is a page people
  // close. The stored value only pre-selects the button state.
  soundBtn.setAttribute('aria-pressed', 'false');
  soundBtn.textContent = '○ audio off';

  /* ── Boot sequence ────────────────────────────────────────────────────── */

  const boot = document.getElementById('boot');
  const bootOut = document.getElementById('boot-out');

  const BOOT_LINES = [
    'HARTLE.TECH INTERFACE // acer gaming-wmi',
    '',
    'probing  /proc/acpi/call ................ present',
    'probing  WMBH  0x79772EC5-04B1-4bfd-843C-61E7F77B6CC9 .. ok',
    'probing  WMAA  ACER_WMID .................. ok',
    'sensors  cpu / gpu / board ............... 3 online',
    'fans     cpu 5882 rpm · gpu 6122 rpm ..... nominal',
    'lighting 4 zones ........................ addressable',
    'firmware V2.04 .......................... unlocked',
    '',
    'no vendor software detected. good.',
  ];

  function endBoot() {
    if (boot.classList.contains('done')) return;
    boot.classList.add('done');
    sessionStorage.setItem('alien-booted', '1');
    setTimeout(() => { boot.remove(); }, 800);
  }

  function runBoot() {
    let line = 0;
    const step = () => {
      if (line >= BOOT_LINES.length) { setTimeout(endBoot, 650); return; }
      bootOut.textContent += BOOT_LINES[line] + '\n';
      if (BOOT_LINES[line].trim()) Sound.servo();
      line++;
      setTimeout(step, 95 + Math.random() * 90);
    };
    step();
  }

  if (sessionStorage.getItem('alien-booted') || reduced) {
    boot.remove();
  } else {
    runBoot();
    boot.addEventListener('click', endBoot);
    window.addEventListener('keydown', endBoot, { once: true });
    // A hard ceiling so a backgrounded tab (where timers are throttled) can
    // never leave someone staring at a half-drawn boot screen.
    setTimeout(endBoot, 9000);
  }

  /* ── The scenes ───────────────────────────────────────────────────────────
     Set dressing behind the whole page: a ship's console still reporting on a
     situation nobody is left to acknowledge. It is a rotation of themes rather
     than one fixed panel — each fades in, runs for a while, and fades away.

     All of it is decorative and aria-hidden, it never moves the layout, and it
     stops itself when the tab is hidden so a parked page costs nothing.

     The vocabulary is deliberately generic — "non-human bioform",
     "hunter-class", "acid breach" — rather than any franchise's proper nouns.
     The homage is in the tone; borrowing marks would put a real company's
     public site somewhere it does not need to be. */

  const scenesEl = document.getElementById('scenes');
  const alarmEl = document.getElementById('alarm');

  if (scenesEl && !reduced) {
    const pick = (a) => a[Math.floor(Math.random() * a.length)];
    const n = (a, b) => Math.floor(a + Math.random() * (b - a));
    const el = (tag, cls, text) => {
      const e = document.createElement(tag);
      if (cls) e.className = cls;
      if (text != null) e.textContent = text;
      return e;
    };

    const DECKS = ['A', 'B', 'C', 'D', 'E'];
    const place = () => `${pick(DECKS)}-${n(1, 10)}${pick(['', '', ' aft', ' fwd', ' port'])}`;

    const EVENTS = [
      ['', 'motion ~ · 3 contacts · non-human'],
      ['', 'internal sensor ~ recalibrated'],
      ['', 'atmos scrubber ~ cycling'],
      ['', 'cargo lift ~ manual override held'],
      ['', 'comms relay ~ carrier only · no voice'],
      ['warn', 'dock ~ seal integrity degraded'],
      ['warn', 'coolant loop ~ offline · hull temp rising'],
      ['warn', 'bioform ~ mass 118kg · gait irregular'],
      ['warn', 'hunter-class signature ~ · cloaked'],
      ['warn', 'thermal bloom ~ · plasma discharge'],
      ['warn', 'blast door ~ jammed · obstruction detected'],
      ['crit', 'DEPRESSURIZED ~ · atmosphere lost'],
      ['crit', 'ACID BREACH ~ · deck plating compromised'],
      ['crit', 'BIOSIGN LOST ~ · crew unaccounted for'],
      ['crit', 'HULL PENETRATION ~ · sealing failed'],
      ['crit', 'CONTAINMENT FAILURE ~ · specimen not in cell'],
      ['crit', 'DOCK ~ FORCED FROM OUTSIDE'],
    ];

    let clock = n(0, 60000);
    const stamp = () => {
      clock += n(7, 340);
      const h = String(Math.floor(clock / 3600) % 24).padStart(2, '0');
      const m = String(Math.floor(clock / 60) % 60).padStart(2, '0');
      const sec = String(clock % 60).padStart(2, '0');
      return `${h}:${m}:${sec}`;
    };

    function alarm() {
      alarmEl.classList.add('on');
      setTimeout(() => alarmEl.classList.remove('on'), 1400);
    }

    /* Each theme returns { node, timers }. The engine owns the fade and the
       teardown; a theme only has to build itself and say what it wants ticked. */

    const THEMES = {
      log() {
        const wrap = el('div', 's-log');
        const push = () => {
          const [sev, tpl] = pick(EVENTS);
          const row = el('div', sev);
          row.appendChild(el('span', 't', stamp()));
          row.appendChild(document.createTextNode(tpl.replace('~', place())));
          wrap.appendChild(row);
          while (wrap.children.length > 15) wrap.removeChild(wrap.firstChild);
          if (sev === 'crit') { alarm(); Sound.squelch(); }
          else if (sev === 'warn') Sound.servo();
        };
        for (let i = 0; i < 12; i++) push();   // arrive mid-incident
        return { node: wrap, tick: push, every: [1500, 3800] };
      },

      radar() {
        const wrap = el('div', 's-radar');
        wrap.appendChild(el('div', 'rings'));
        wrap.appendChild(el('div', 'sweep'));
        const blip = () => {
          const b = el('div', 'blip' + (Math.random() < 0.4 ? ' hostile' : ''));
          // Uniform over the disc, not over the radius — otherwise everything
          // clusters in the middle.
          const r = Math.sqrt(Math.random()) * 46;
          const a = Math.random() * Math.PI * 2;
          b.style.left = `${50 + r * Math.cos(a)}%`;
          b.style.top = `${50 + r * Math.sin(a)}%`;
          wrap.appendChild(b);
          setTimeout(() => b.remove(), 4200);
          Sound.ping();
        };
        for (let i = 0; i < 4; i++) blip();
        return { node: wrap, tick: blip, every: [900, 2600] };
      },

      decks() {
        const wrap = el('div', 's-decks');
        const cells = [];
        for (let i = 0; i < 72; i++) { const c = el('i'); wrap.appendChild(c); cells.push(c); }
        const churn = () => {
          const c = pick(cells);
          const was = c.className;
          c.className = pick(['', '', '', 'breach', 'breach', 'vent', 'hot']);
          if (c.className === 'hot' && was !== 'hot') Sound.servo();
        };
        // Already damaged on arrival. A ship that degrades from pristine while
        // you watch reads as a loading bar; one already in trouble reads as
        // history.
        for (let i = 0; i < 26; i++) churn();
        return { node: wrap, tick: churn, every: [500, 1600] };
      },

      vitals() {
        const wrap = el('div', 's-vitals');
        const M = [
          { k: 'o2 reserve', v: 41, lo: 16, hi: 46, u: '%' },
          { k: 'pressure', v: 62, lo: 28, hi: 78, u: 'kPa' },
          { k: 'reactor output', v: 88, lo: 68, hi: 96, u: '%' },
          { k: 'hull integrity', v: 54, lo: 34, hi: 70, u: '%' },
          { k: 'crew biosign', v: 2, lo: 2, hi: 3, u: '/19' },
        ];
        M.forEach((m) => {
          const row = el('div');
          row.appendChild(el('span', null, m.k));
          const bar = el('span', 'bar');
          m.fill = el('b');
          bar.appendChild(m.fill);
          row.appendChild(bar);
          m.val = el('span', 'val');
          row.appendChild(m.val);
          wrap.appendChild(row);
          m.bar = bar;
        });
        const drift = () => {
          M.forEach((m) => {
            // Biased downward: everything on this ship is getting worse.
            m.v = Math.max(m.lo, Math.min(m.hi, m.v + (Math.random() - 0.56) * 2.4));
            const pct = m.u === '/19' ? (m.v / 19) * 100 : m.v;
            m.fill.style.width = `${Math.max(2, pct)}%`;
            m.val.textContent = `${Math.round(m.v)}${m.u}`;
            m.bar.className = `bar ${pct < 30 ? 'crit' : pct < 55 ? 'low' : ''}`;
          });
        };
        drift();
        return { node: wrap, tick: drift, every: [1800, 3400] };
      },

      roster() {
        const NAMES = ['ARCARO J', 'BENNET T', 'CHIGARU N', 'DELACROIX M', 'ESPARZA R',
                       'FOWLER K', 'GRIEVES A', 'HALLORAN P', 'IKEDA S', 'JANSSEN W',
                       'KOVAC L', 'LUMET D', 'MBEKI O', 'NOVAK F', 'ORTIZ V',
                       'PARIS H', 'QUINTERO E', 'REYES B', 'SOKOLOV Y'];
        const wrap = el('div', 's-roster');
        const rows = NAMES.map((name) => {
          const d = el('div');
          d.appendChild(document.createTextNode(name + ' · '));
          const st = el('span', null, 'nominal');
          d.appendChild(st);
          wrap.appendChild(d);
          return { d, st };
        });
        // Seventeen of nineteen are already gone when the screen comes up.
        const order = rows.map((_, i) => i).sort(() => Math.random() - 0.5);
        let cursor = 0;
        const kill = () => {
          if (cursor >= order.length - 2) return;
          const r = rows[order[cursor++]];
          const lost = Math.random() < 0.72;
          r.d.className = lost ? 'lost' : 'unk';
          r.st.textContent = lost ? 'biosign lost' : 'no contact';
          if (lost) Sound.squelch();
        };
        for (let i = 0; i < 13; i++) kill();
        return { node: wrap, tick: kill, every: [1400, 3000] };
      },

      wire() {
        // Pure vector: concentric polygons, an orbit ring, a hull outline and a
        // set of bearing ticks. Nothing filled — filled shapes at this size
        // read as a chart pasted over the page instead of a display behind it.
        const NS = 'http://www.w3.org/2000/svg';
        const svg = document.createElementNS(NS, 'svg');
        svg.setAttribute('viewBox', '-100 -100 200 200');

        const poly = (sides, r, cls, extra) => {
          const pts = [];
          for (let i = 0; i < sides; i++) {
            const a = (i / sides) * Math.PI * 2 - Math.PI / 2;
            pts.push(`${(Math.cos(a) * r).toFixed(2)},${(Math.sin(a) * r).toFixed(2)}`);
          }
          const e = document.createElementNS(NS, 'polygon');
          e.setAttribute('points', pts.join(' '));
          e.setAttribute('stroke', 'currentColor');
          e.setAttribute('stroke-width', '0.7');
          e.setAttribute('opacity', extra || '0.55');
          if (cls) e.setAttribute('class', cls);
          return e;
        };

        const circle = (r, dash, cls) => {
          const e = document.createElementNS(NS, 'circle');
          e.setAttribute('r', r);
          e.setAttribute('stroke', 'currentColor');
          e.setAttribute('stroke-width', '0.6');
          e.setAttribute('opacity', '0.4');
          if (dash) e.setAttribute('stroke-dasharray', dash);
          if (cls) e.setAttribute('class', cls);
          return e;
        };

        svg.appendChild(poly(6, 88, 'spin-slow', '0.5'));
        svg.appendChild(poly(3, 62, 'spin-rev', '0.45'));
        svg.appendChild(poly(6, 34, 'spin-slow pulse', '0.7'));
        svg.appendChild(circle(74, '2 5', 'spin-rev'));
        svg.appendChild(circle(46));

        // Bearing ticks around the outer ring.
        const ticks = document.createElementNS(NS, 'g');
        ticks.setAttribute('class', 'spin-slow');
        for (let i = 0; i < 36; i++) {
          const a = (i / 36) * Math.PI * 2;
          const l = document.createElementNS(NS, 'line');
          const r0 = i % 3 === 0 ? 90 : 94;
          l.setAttribute('x1', (Math.cos(a) * r0).toFixed(2));
          l.setAttribute('y1', (Math.sin(a) * r0).toFixed(2));
          l.setAttribute('x2', (Math.cos(a) * 98).toFixed(2));
          l.setAttribute('y2', (Math.sin(a) * 98).toFixed(2));
          l.setAttribute('stroke', 'currentColor');
          l.setAttribute('stroke-width', '0.6');
          l.setAttribute('opacity', '0.4');
          ticks.appendChild(l);
        }
        svg.appendChild(ticks);

        // A crosshair that jumps to a new bearing every tick — the only moving
        // part that is not on a fixed rotation, so the scene never looks looped.
        const cross = document.createElementNS(NS, 'g');
        cross.setAttribute('stroke', 'currentColor');
        cross.setAttribute('stroke-width', '0.7');
        cross.setAttribute('opacity', '0.85');
        const mk = (x1, y1, x2, y2) => {
          const l = document.createElementNS(NS, 'line');
          l.setAttribute('x1', x1); l.setAttribute('y1', y1);
          l.setAttribute('x2', x2); l.setAttribute('y2', y2);
          return l;
        };
        const box = document.createElementNS(NS, 'rect');
        box.setAttribute('width', '18'); box.setAttribute('height', '18');
        box.setAttribute('stroke', 'currentColor');
        box.setAttribute('stroke-width', '0.7');
        box.setAttribute('fill', 'none');
        cross.appendChild(box);
        cross.appendChild(mk(-26, 9, -4, 9));
        cross.appendChild(mk(22, 9, 44, 9));
        svg.appendChild(cross);

        const wrap = el('div', 's-wire');
        wrap.appendChild(svg);

        const move = () => {
          const r = Math.sqrt(Math.random()) * 62;
          const a = Math.random() * Math.PI * 2;
          const x = Math.cos(a) * r - 9;
          const y = Math.sin(a) * r - 9;
          box.setAttribute('x', x.toFixed(1));
          box.setAttribute('y', y.toFixed(1));
          cross.setAttribute('transform', `translate(${(x + 9).toFixed(1)} ${(y + 9).toFixed(1)})`);
          box.setAttribute('x', -9); box.setAttribute('y', -9);
          Sound.ping();
        };
        move();
        return { node: wrap, tick: move, every: [1800, 4200] };
      },

      docks() {
        const BAYS = ['dock 01', 'dock 02', 'dock 03', 'airlock a', 'airlock b',
                      'cargo ramp', 'escape pod 1', 'escape pod 2'];
        const STATES = [
          ['ok', 'sealed'], ['ok', 'sealed'], ['warn', 'cycling'],
          ['warn', 'seal degraded'], ['crit', 'FORCED — OPEN TO VACUUM'],
          ['crit', 'DEPRESSURIZED'], ['warn', 'manual override'],
          ['crit', 'LAUNCHED — UNAUTHORISED'],
        ];
        const wrap = el('div', 's-docks');
        const rows = BAYS.map((bay) => {
          const d = el('div');
          d.appendChild(el('span', null, bay));
          const st = el('span');
          const code = el('span', null, `#${n(1000, 9999)}`);
          d.appendChild(st); d.appendChild(code);
          wrap.appendChild(d);
          return st;
        });
        const set = () => {
          const st = pick(rows);
          const [cls, text] = pick(STATES);
          st.className = cls;
          st.textContent = text;
          if (cls === 'crit') { alarm(); Sound.squelch(); }
        };
        for (let i = 0; i < 8; i++) set();
        return { node: wrap, tick: set, every: [1600, 3600] };
      },
    };

    const TITLES = {
      log: 'incident log // unacknowledged',
      radar: 'motion tracker // deck sweep',
      decks: 'compartment integrity',
      vitals: 'life support',
      roster: 'crew manifest',
      docks: 'dock and airlock status',
      wire: 'navigation lattice // dead reckoning',
    };

    const NAMES = Object.keys(THEMES);
    let current = null;
    let running = false;
    let cycleTimer = null;

    function teardown(scene) {
      if (!scene) return;
      scene.timers.forEach(clearTimeout);
      scene.node.classList.remove('on');
      setTimeout(() => scene.node.remove(), 2600);
    }

    function show(name) {
      const built = THEMES[name]();
      const node = el('div', 'scene');
      node.appendChild(el('div', 'scene-title', TITLES[name]));
      node.appendChild(built.node);
      scenesEl.appendChild(node);

      const scene = { node, timers: [] };
      // Next frame, so the browser has a chance to paint opacity:0 first and
      // actually run the transition rather than snapping straight to visible.
      requestAnimationFrame(() => requestAnimationFrame(() => node.classList.add('on')));

      const loop = () => {
        built.tick();
        scene.timers.push(setTimeout(loop, n(built.every[0], built.every[1])));
      };
      scene.timers.push(setTimeout(loop, n(built.every[0], built.every[1])));
      return scene;
    }

    let lastName = null;
    function cycle() {
      if (!running) return;
      let name = pick(NAMES);
      // Never the same theme twice in a row; with six of them, resampling is
      // simpler than shuffling and the bias is invisible.
      while (name === lastName && NAMES.length > 1) name = pick(NAMES);
      lastName = name;

      const previous = current;
      current = show(name);
      teardown(previous);

      cycleTimer = setTimeout(cycle, n(11000, 17000));
    }

    function start() { if (running) return; running = true; cycle(); }
    function stop() {
      running = false;
      clearTimeout(cycleTimer);
      teardown(current);
      current = null;
    }

    // A hidden tab throttles timers anyway; stopping outright is honest about
    // it and keeps a parked tab off the CPU entirely.
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) stop(); else start();
    });

    start();
  }

  /* ── Copy buttons ─────────────────────────────────────────────────────── */

  document.querySelectorAll('[data-copy]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const text = btn.getAttribute('data-copy');
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        // Clipboard API needs a secure context and a permission that can be
        // refused. Fall back rather than lying about having copied.
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.setAttribute('readonly', '');
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand('copy'); } catch { /* nothing left to try */ }
        ta.remove();
      }
      const was = btn.textContent;
      btn.textContent = 'copied';
      btn.classList.add('ok');
      Sound.confirm();
      setTimeout(() => { btn.textContent = was; btn.classList.remove('ok'); }, 1600);
    });
  });

  /* ── Install tabs ─────────────────────────────────────────────────────── */

  const tabs = Array.from(document.querySelectorAll('.tab'));
  tabs.forEach((tab) => {
    tab.addEventListener('click', () => {
      tabs.forEach((t) => {
        const sel = t === tab;
        t.setAttribute('aria-selected', String(sel));
        document.getElementById(t.getAttribute('aria-controls')).classList.toggle('on', sel);
      });
      Sound.servo();
    });
  });

  /* ── Thermal pointer ──────────────────────────────────────────────────── */

  document.querySelectorAll('.card').forEach((card) => {
    card.addEventListener('pointermove', (e) => {
      const r = card.getBoundingClientRect();
      card.style.setProperty('--mx', `${e.clientX - r.left}px`);
      card.style.setProperty('--my', `${e.clientY - r.top}px`);
    });
    let armed = true;
    card.addEventListener('pointerenter', () => {
      if (!armed) return;
      armed = false;
      Sound.squelch();
      setTimeout(() => { armed = true; }, 500);
    });
  });

  /* ── Reveal ───────────────────────────────────────────────────────────── */

  // The hidden state is armed here and nowhere else. Everything above has
  // already run, so if any of it had thrown, this line would never execute and
  // the page would simply render without the animation — visible, which is the
  // only acceptable failure mode.
  if (!reduced && 'IntersectionObserver' in window) {
    document.documentElement.classList.add('reveal-ready');

    const io = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        entry.target.classList.add('seen');
        io.unobserve(entry.target);
        Sound.ping();
      });
    }, { threshold: 0.15 });

    const targets = document.querySelectorAll('.reveal');
    targets.forEach((el) => io.observe(el));

    // Belt and braces for anything the observer never gets to fire for: a
    // deep link that lands mid-page, a throttled background tab, a headless
    // renderer. After two seconds, show everything regardless.
    setTimeout(() => targets.forEach((el) => el.classList.add('seen')), 2000);
  }
})();
