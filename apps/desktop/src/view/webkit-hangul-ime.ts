import type { Terminal } from '@xterm/xterm';

const HANGUL_RANGES: ReadonlyArray<readonly [number, number]> = [
  [0x1100, 0x11ff],
  [0x3130, 0x318f],
  [0xa960, 0xa97f],
  [0xac00, 0xd7af],
  [0xd7b0, 0xd7ff],
];

/**
 * Adapt WKWebView's non-standard Korean IME events to xterm input.
 *
 * WKWebView sends an initial jamo as insertText, then replaces it with a
 * composed syllable through insertReplacementText. xterm 6.0 ignores the
 * replacement events and forwards only the raw jamo. This adapter buffers the
 * latest replacement and commits it before the next character or command key.
 *
 * Mirrors the behavior proposed in xterm.js PR #5704. Other browser engines
 * keep xterm's native composition path.
 */
export function installWebKitHangulImeAdapter(
  terminal: Terminal,
  send: (data: string) => void,
): () => void {
  if (!isWebKit() || !terminal.textarea) return () => undefined;

  const textarea = terminal.textarea;
  const element = terminal.element;
  if (!element) return () => undefined;
  let pending = '';
  let sawReplacement = false;
  let compositionEnded = false;
  let keydownAfterCompositionEnd = false;
  let compositionFlushTimer: ReturnType<typeof setTimeout> | undefined;

  const hidePreview = (): void => {
    const preview = terminal.element?.querySelector<HTMLElement>('.composition-view');
    if (!preview) return;
    preview.textContent = '';
    preview.classList.remove('active');
  };

  const showPreview = (text: string): void => {
    const element = terminal.element;
    const preview = element?.querySelector<HTMLElement>('.composition-view');
    const screen = element?.querySelector<HTMLElement>('.xterm-screen');
    if (!preview || !screen) return;

    const cellWidth = screen.clientWidth / terminal.cols;
    const cellHeight = screen.clientHeight / terminal.rows;
    const buffer = terminal.buffer.active;
    preview.textContent = text;
    preview.classList.add('active');
    preview.style.left = `${Math.min(buffer.cursorX, terminal.cols - 1) * cellWidth}px`;
    preview.style.top = `${buffer.cursorY * cellHeight}px`;
    preview.style.height = `${cellHeight}px`;
    preview.style.lineHeight = `${cellHeight}px`;
    preview.style.fontFamily = terminal.options.fontFamily ?? '';
    preview.style.fontSize = `${terminal.options.fontSize}px`;
  };

  const flush = (): void => {
    if (compositionFlushTimer !== undefined) {
      clearTimeout(compositionFlushTimer);
      compositionFlushTimer = undefined;
    }
    if (!pending) return;
    const text = pending;
    pending = '';
    sawReplacement = false;
    hidePreview();
    send(text);
  };

  const scheduleFlush = (): void => {
    if (compositionFlushTimer !== undefined) clearTimeout(compositionFlushTimer);
    compositionFlushTimer = setTimeout(() => {
      compositionFlushTimer = undefined;
      flush();
    }, 0);
  };

  const handleInput = (event: Event): void => {
    if (event.target !== textarea) return;
    const input = event as InputEvent;
    if (input.inputType === 'deleteCompositionText') {
      pending = '';
      hidePreview();
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }

    if (!input.data) return;

    // WebKit can emit a final insertText after compositionend even though the
    // same text was already committed by the deferred flush. It has no matching
    // keydown; swallowing it prevents duplicated Korean syllables.
    if (compositionEnded && !keydownAfterCompositionEnd
      && input.inputType === 'insertText' && isHangul(input.data)) {
      compositionEnded = false;
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }
    compositionEnded = false;

    if (input.inputType === 'insertCompositionText' || input.inputType === 'insertFromComposition') {
      pending = input.data;
      sawReplacement = true;
      showPreview(pending);
      if (input.inputType === 'insertFromComposition') scheduleFlush();
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }

    if (input.inputType === 'insertReplacementText') {
      pending = input.data;
      sawReplacement = true;
      showPreview(pending);
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }

    if (input.inputType === 'insertText' && isHangul(input.data)) {
      if (sawReplacement && pending === input.data) {
        event.preventDefault();
        event.stopImmediatePropagation();
        return;
      }
      if (pending && pending !== input.data) flush();
      pending = input.data;
      sawReplacement = false;
      showPreview(pending);
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }

    // Let xterm process the new input normally, but commit Hangul first.
    flush();
  };

  const handleKeydown = (event: KeyboardEvent): void => {
    if (event.target !== textarea) return;
    // Modifier-only keydowns do not commit text. In particular, flushing on
    // Shift split doubled consonants such as ㄲ/ㅆ into separate syllables.
    if (event.key === 'Shift' || event.key === 'Control' || event.key === 'Alt'
      || event.key === 'Meta' || event.key === 'CapsLock') return;
    if (compositionEnded) keydownAfterCompositionEnd = true;
    // Escape is handled by TerminalPane's custom key handler even when the IME
    // reports keyCode 229. Let it through after committing the current syllable.
    if (event.key === 'Escape') {
      flush();
      return;
    }
    const isControlCommand = event.ctrlKey || event.metaKey || event.altKey;
    if (event.keyCode === 229 && !isControlCommand) {
      // WKWebView dispatches this keydown before the first Hangul input event,
      // so `pending` is still empty for the initial jamo. Always suppress the
      // event or xterm schedules _handleAnyTextareaChanges and sends that raw
      // jamo in addition to the composed syllable.
      event.stopImmediatePropagation();
      return;
    }
    if (!pending) return;
    // Space commits an active Hangul composition. xterm sends the space from
    // keydown, before the following input event, so flush here to preserve the
    // actual "syllable then space" order.
    if (event.key === ' ') {
      flush();
      return;
    }
    // WKWebView does not always mark Hangul keydowns as keyCode 229. A
    // printable key can still update the current syllable, so let its input
    // event decide whether to replace the syllable or begin the next one.
    if (event.key.length === 1 && !isControlCommand) {
      // xterm emits punctuation from keydown. Flush first so `건데.` cannot
      // arrive at the PTY as `건.데`.
      if (!isHangul(event.key)) flush();
      return;
    }
    // Commit the completed syllable before Enter, arrows, shortcuts, etc.
    flush();
  };

  const blockNativeComposition = (event: Event): void => {
    if (event.target !== textarea) return;
    const composition = event as CompositionEvent;
    event.stopImmediatePropagation();
    if (event.type === 'compositionstart') {
      // A new syllable may start before the previous compositionend timer gets
      // a turn. Commit the previous syllable now so the timer cannot flush the
      // new syllable by mistake.
      flush();
      compositionEnded = false;
      keydownAfterCompositionEnd = false;
    }
    if (composition.data && isHangul(composition.data)) {
      pending = composition.data;
      showPreview(pending);
    }
    // Some WebKit versions dispatch a final input event after compositionend.
    // Defer the commit one turn so that event can update `pending` first.
    if (event.type === 'compositionend') {
      compositionEnded = true;
      keydownAfterCompositionEnd = false;
      scheduleFlush();
    }
  };

  // Listen on xterm's root in the capture phase. An ancestor capture listener
  // runs before xterm's listeners on the textarea, so the raw initial jamo
  // cannot be emitted before we see and replace it with the composed syllable.
  element.addEventListener('input', handleInput, { capture: true });
  element.addEventListener('keydown', handleKeydown, { capture: true });
  element.addEventListener('blur', flush, { capture: true });
  element.addEventListener('compositionstart', blockNativeComposition, { capture: true });
  element.addEventListener('compositionupdate', blockNativeComposition, { capture: true });
  element.addEventListener('compositionend', blockNativeComposition, { capture: true });

  return () => {
    if (compositionFlushTimer !== undefined) clearTimeout(compositionFlushTimer);
    element.removeEventListener('input', handleInput, { capture: true });
    element.removeEventListener('keydown', handleKeydown, { capture: true });
    element.removeEventListener('blur', flush, { capture: true });
    element.removeEventListener('compositionstart', blockNativeComposition, { capture: true });
    element.removeEventListener('compositionupdate', blockNativeComposition, { capture: true });
    element.removeEventListener('compositionend', blockNativeComposition, { capture: true });
    hidePreview();
  };
}

function isHangul(text: string): boolean {
  const codePoint = text.codePointAt(0);
  return codePoint !== undefined && HANGUL_RANGES.some(([start, end]) => codePoint >= start && codePoint <= end);
}

function isWebKit(): boolean {
  const userAgent = navigator.userAgent;
  return /AppleWebKit/i.test(userAgent) && !/(Chrome|Chromium|Edg)/i.test(userAgent);
}
