import { OriginViewInstruction, TypstPatchAttrs, changeViewPerspective, equalPatchElem, interpretTargetView, patchAttributes } from "./svg-patch";

interface CursorPosition {
  // eslint-disable-next-line @typescript-eslint/naming-convention
  page_no: number,
  x: number,
  y: number,
}

export interface OutlineItemData {
  title: string,
  position?: CursorPosition,
  children: OutlineItemData[],
}

type GenNode = CanvasPage | GenElem;

export interface CanvasPage {
  index: number;
  width: number;
  height: number;
  container: HTMLElement;
  elem: HTMLElement;

  // extra properties for patching
  inserter?: (t: CanvasPage) => void,
  stub?: HTMLElement,
}

class GenElem {
  children: GenNode[] = [];
  constructor(public container: HTMLElement) { }

  push(child: GenNode) {
    this.children.push(child);
    this.container.append(child.container);
  }

  pushCanvas(pg: CanvasPage) {
    const stub = document.createElement('div');
    stub.setAttribute(TypstPatchAttrs.Tid, `canvas:` + pg.index);
    // stubCanvas.setAttribute(TypstPatchAttrs.ReuseFrom, `canvas:` + pg.index);
    stub.setAttribute('data-page-number', pg.index.toString());
    pg.stub = stub;

    this.children.push(pg);
    this.container.append(stub);
  }
}

function tagPatchId(elem: HTMLElement, tid: string) {
  elem.setAttribute(TypstPatchAttrs.Tid, tid);
  elem.setAttribute(TypstPatchAttrs.ReuseFrom, tid);
  elem.setAttribute(TypstPatchAttrs.BadEquality, '1');
}

function poisionCanvasMoved(t: CanvasPage) {
  console.error('never called moved canvas', t);
  throw new Error('never called moved canvas');
}

function replaceStubToRealCanvas(t: CanvasPage) {
  // console.log('move', t.stub!.outerHTML, 'to', t.container.outerHTML);
  t.stub!.replaceWith(t.container);
  t.stub = undefined;
}

class GenContext {
  populateCnt: number = 1;
  insertionPoint: GenElem;
  parent: GenElem;
  lastVisit?: GenElem;

  constructor(public pages: CanvasPage[]) {
    this.insertionPoint = new GenElem(document.createElement('div'));
    this.parent = this.insertionPoint;
  }

  /// Populate canvas stubs from `this.populateCnt` to `until` (exclusive).
  spliceCanvas(next: GenElem, until: number) {
    until = Math.min(until, this.pages.length + 1);
    for (let i = this.populateCnt; i < until; i++) {
      next.pushCanvas(this.pages[i - 1]);
    }
    this.populateCnt = Math.max(until, this.populateCnt);
  }

  /// Generate outline node for `item` and its children.
  generate(item: OutlineItemData, level: number): GenElem {
    // console.log(`g page_no: ${item.position?.page_no}`, ctx.populateCnt, item);

    const outlineDiv = document.createElement('div');
    outlineDiv.classList.add('typst-outline');
    outlineDiv.setAttribute('data-title', item.title);
    tagPatchId(outlineDiv, 'outline:' + item.title);
    const outlineNode = new GenElem(outlineDiv);

    let pos = item.position?.page_no || 0;
    let hasChildren = Math.max(0, pos - this.populateCnt) + item.children.length > 0;

    // populate canvas stubs before this node
    this.spliceCanvas(this.insertionPoint, pos);

    // create title at the beginning of this node
    const titleSpan = document.createElement('span');
    titleSpan.classList.add('typst-outline-title', 'level-' + level);
    titleSpan.textContent = item.title;
    tagPatchId(titleSpan, 'title:' + item.title);
    outlineNode.push(new GenElem(titleSpan));

    // pre-order traversal last visit
    this.lastVisit = outlineNode;
    this.parent.push(outlineNode);

    // stacked save insertion point and parent
    const parent = this.parent;
    const insertionPoint = this.insertionPoint;
    this.parent = outlineNode;
    this.insertionPoint = outlineNode;

    for (const ch of item.children) {
      this.insertionPoint = this.generate(ch, level + 1);
    }

    this.insertionPoint = insertionPoint;
    this.parent = parent;

    // apply clickable behavior to node containing children
    if (hasChildren) {
      titleSpan.style.textDecoration = 'underline';
      titleSpan.style.cursor = 'pointer';

      titleSpan.addEventListener('click', () => {
        titleSpan.parentElement!.classList.toggle('collapsed');
      });
    }

    return outlineNode;
  }
}

/// Receiving a sequence of canvas pages, and a sequence of outline items
/// Produce or patch the outline element to the `prev` container.
export function patchOutlineEntry(prev: HTMLDivElement, pages: CanvasPage[], items: OutlineItemData[]) {
  const ctx = new GenContext(pages);
  // the root element of the generated outline
  const next = ctx.insertionPoint;

  // generate outline
  for (const item of items) {
    ctx.insertionPoint = ctx.generate(item, 1);
  }
  // populate canvas stubs after the last node
  ctx.spliceCanvas(ctx.lastVisit || next, pages.length + 1);

  // patch outline to container
  if (prev.children.length === 0) {
    // newly created outline
    prev.append(...next.container.children);
  } else {
    // patch existing outline
    patchOutlineChildren(ctx, prev, next.container);
  }

  for (const page of pages) {
    // all of stubs is already inserted to the dom, so we just 
    page.inserter ||= replaceStubToRealCanvas;
  }
}

/// Replace the `prev` element with `next` element.
/// Return true if the `prev` element is reused.
/// Return false if the `prev` element is replaced.
function reuseOrPatchOutlineElem(ctx: GenContext, prev: Element, next: Element) {
  const canReuse = equalPatchElem(prev, next);

  /// Even if the element is reused, we still need to replace its attributes.
  next.removeAttribute(TypstPatchAttrs.ReuseFrom);
  const isPageElem = prev.classList.contains('typst-page');
  if (!isPageElem) {
    patchAttributes(prev, next);
  }

  if (canReuse) {
    if (isPageElem) {
      const pageNumber = Number.parseInt(next.getAttribute('data-page-number')!);
      // console.log('reuse canvas', ctx.pages[pageNumber], prev, next);
      const page = ctx.pages[pageNumber];
      page.inserter = poisionCanvasMoved;

      page.container = prev as HTMLElement;
      page.elem = page.container.firstElementChild as HTMLElement;
    }
    return true /* reused */;
  } else if (isPageElem) {
    // will never dive into the internals of a canvas element
    return false;
  }

  /// Patch `<div>` children, will call `reuseOrPatchElem` again.
  patchOutlineChildren(ctx, prev, next);
  return false /* reused */;
}

/// apply patches to the children sequence of `prev outline` in the DOM
function patchOutlineChildren(ctx: GenContext, prev: Element, next: Element) {
  const [targetView, toPatch] = interpretTargetView<Element>(
    prev.children as unknown as Element[],
    next.children as unknown as Element[],
    // todo: accurate calcuation
    false,
  );

  // console.log("interpreted origin view", targetView, toPatch);

  for (let [prevChild, nextChild] of toPatch) {
    reuseOrPatchOutlineElem(ctx, prevChild, nextChild);
  }

  // console.log("interpreted target view", targetView);

  const originView = changeViewPerspective(
    prev.children as unknown as Element[],
    targetView,
  );

  runOriginViewInstructionsOnOutline(ctx, prev, originView);
}

function runOriginViewInstructionsOnOutline(
  ctx: GenContext, prev: Element, originView: OriginViewInstruction<Node>[]
) {
  // console.log("interpreted origin view", originView);
  for (const [op, off, fr] of originView) {
    const elem = prev.children[off];
    switch (op) {
      case "insert":
        prev.insertBefore(fr, elem);
        break;
      case "swap_in":
        prev.insertBefore(prev.children[fr], elem);
        break;
      case "remove":
        if (elem?.classList?.contains('typst-page')) {
          const pageNumber = Number.parseInt(elem.getAttribute('data-page-number')!);
          if (pageNumber < ctx.pages.length) {
            const page = ctx.pages[pageNumber];
            // console.log('recover canvas', page, pageNumber);

            // recover the removed page, and we could reuse it later
            page.container = elem as HTMLElement;
            page.elem = page.container.firstElementChild as HTMLElement;
          }
        }
        elem.remove();
        break;
      default:
        throw new Error("unknown op " + op);
    }
  }
}
