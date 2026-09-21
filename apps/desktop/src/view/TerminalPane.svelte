<script lang="ts">
  import { onMount } from 'svelte';
  import { FitAddon } from '@xterm/addon-fit';
  import { Terminal } from '@xterm/xterm';
  import '@xterm/xterm/css/xterm.css';
  import { installWebKitHangulImeAdapter } from './webkit-hangul-ime';

  export let sessionId: string;
  export let registerOutput: (sessionId: string, sink: (data: string, replay?: boolean) => void) => () => void;
  export let onInput: (sessionId: string, data: string) => void;
  export let onResize: (sessionId: string, cols: number, rows: number) => void;
  export let onInterrupt: (sessionId: string) => void;
  export let onFocus: (sessionId: string) => void;
  export let onCurrentDirectory: (sessionId: string, path: string) => void;
  export let fontSize = 11;

  let container: HTMLDivElement;
  let terminalRef: Terminal | undefined;
  let fitRef: FitAddon | undefined;

  $: if (terminalRef && terminalRef.options.fontSize !== fontSize) {
    terminalRef.options.fontSize = fontSize;
    try { fitRef?.fit(); } catch { /* hidden terminal; retry on next resize */ }
  }

  onMount(() => {
    const terminal = new Terminal({
      allowProposedApi: false,
      cursorBlink: true,
      convertEol: false,
      fontFamily: 'SFMono-Regular, Menlo, Monaco, Consolas, "Apple SD Gothic Neo", "Noto Sans Mono CJK KR", monospace',
      fontSize,
      lineHeight: 1.15,
      scrollback: 5000,
      theme: {
        background: '#0b100d',
        foreground: '#d6e0d9',
        cursor: '#8eefaa',
        selectionBackground: '#385744',
      },
    });
    terminalRef = terminal;
    const fit = new FitAddon();
    fitRef = fit;
    terminal.loadAddon(fit);
    terminal.open(container);
    const removeHangulImeAdapter = installWebKitHangulImeAdapter(
      terminal,
      (data) => onInput(sessionId, data),
    );
    let replayWrites = 0;
    const unregister = registerOutput(sessionId, (data, replay = false) => {
      if (!replay) {
        terminal.write(data);
        return;
      }
      replayWrites += 1;
      terminal.write(data, () => { replayWrites -= 1; });
    });
    const input = terminal.onData((data) => {
      // xterm emits replies to OSC color, cursor-position, and device-attribute
      // queries through onData. Queries in a historical transcript must not
      // inject those replies into the live shell when the pane is mounted.
      if (replayWrites === 0) onInput(sessionId, data);
    });
    const handlePaste = (event: ClipboardEvent): void => {
      if (event.target !== terminal.textarea) return;
      const text = event.clipboardData?.getData('text/plain');
      if (!text) return;
      // WKWebView does not reliably complete xterm's hidden-textarea paste
      // path. Feed the user-initiated clipboard payload through xterm so its
      // bracketed-paste handling remains intact.
      event.preventDefault();
      event.stopImmediatePropagation();
      terminal.paste(text);
    };
    terminal.element?.addEventListener('paste', handlePaste, { capture: true });
    // Private OSC number (must match FASTADE_CWD_OSC in app-view-model.svelte.ts),
    // not the standard OSC 7 "report cwd" code — an AI CLI running inside this
    // shell (Claude Code, Codex, Gemini) may itself emit real OSC 7 for its own
    // terminal integration, which would otherwise be misread as "the outer
    // shell's prompt came back" and falsely mark a still-running agent as exited.
    const cwd = terminal.parser.registerOscHandler(55123, (data) => {
      if (!data) return false;
      onCurrentDirectory(sessionId, data);
      return true;
    });
    const resized = terminal.onResize(({ cols, rows }) => {
      // A session can be visible in both the main window and a pop-out. Only
      // the focused window owns the live PTY size; otherwise two observers
      // continually overwrite each other with different rows and columns.
      if (document.hasFocus()) onResize(sessionId, cols, rows);
    });
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.key !== 'Escape' || event.type !== 'keydown') return true;
      event.preventDefault();
      event.stopPropagation();
      onInterrupt(sessionId);
      return false;
    });
    let fitFrame = 0;
    const scheduleFit = (): void => {
      cancelAnimationFrame(fitFrame);
      fitFrame = requestAnimationFrame(() => {
        // A second frame observes the final content size after macOS finishes
        // a fullscreen animation or moves the window between display scales.
        fitFrame = requestAnimationFrame(() => {
          try { fit.fit(); } catch { /* hidden terminal; retry on next resize */ }
        });
      });
    };
    const observer = new ResizeObserver(scheduleFit);
    let resolutionQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    const handleResolutionChange = (): void => {
      resolutionQuery.removeEventListener('change', handleResolutionChange);
      resolutionQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      resolutionQuery.addEventListener('change', handleResolutionChange);
      scheduleFit();
    };
    observer.observe(container);
    window.addEventListener('resize', scheduleFit);
    window.addEventListener('focus', scheduleFit);
    window.visualViewport?.addEventListener('resize', scheduleFit);
    resolutionQuery.addEventListener('change', handleResolutionChange);
    scheduleFit();
    requestAnimationFrame(() => terminal.focus());
    return () => {
      cancelAnimationFrame(fitFrame);
      observer.disconnect();
      window.removeEventListener('resize', scheduleFit);
      window.removeEventListener('focus', scheduleFit);
      window.visualViewport?.removeEventListener('resize', scheduleFit);
      resolutionQuery.removeEventListener('change', handleResolutionChange);
      removeHangulImeAdapter();
      unregister();
      input.dispose();
      terminal.element?.removeEventListener('paste', handlePaste, { capture: true });
      cwd.dispose();
      resized.dispose();
      terminal.dispose();
      terminalRef = undefined;
      fitRef = undefined;
    };
  });
</script>

<div
  class="terminal-host"
  role="application"
  aria-label="Terminal"
  bind:this={container}
  onfocusin={() => onFocus(sessionId)}
  oncontextmenu={(event) => event.stopPropagation()}
></div>

<style>
  /* Keep the final TUI status row clear of WebKit's clipped bottom edge.
     FitAddon only subtracts padding from `.xterm` itself, not its parent.
     Keeping the padding on the host made the PTY one row and a few columns
     larger than the visible screen, which displaced Codex's composer cursor
     and caused bottom-anchored choice prompts to overlap their content. */
  .terminal-host { width: 100%; height: 100%; min-width: 0; min-height: 0; overflow: hidden; background: #0b100d; }
  :global(.xterm) { width: 100%; height: 100%; padding: 0 0 11px; }
  :global(.xterm-viewport) { scrollbar-width: thin; }
</style>
