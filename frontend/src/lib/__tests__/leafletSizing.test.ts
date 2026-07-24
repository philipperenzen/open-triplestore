import { describe, it, expect } from 'vitest';
import L from '../viewer/leafletIcons';

// These assertions pin down the Leaflet behaviour that GeoPreview's deferred
// first draw is built around. jsdom reports every element as 0×0, which makes it
// an exact model of "the container has not been laid out yet" — the state a map
// is in inside a panel that is still sizing, or a tab that has not been revealed.
//
// If a future Leaflet release makes any of this stop being true, these tests
// fail: that is the signal to go simplify GeoPreview.drawAll(), not to loosen
// the assertions.
describe('leaflet size caching (why GeoPreview measures the element, not the map)', () => {
  function freshMap() {
    const el = document.createElement('div');
    document.body.appendChild(el);
    return { el, map: L.map(el, { scrollWheelZoom: false, attributionControl: true }) };
  }

  it('does not measure the container until something asks for the size', () => {
    // This is the property that makes deferring safe: as long as nothing calls
    // getSize() early, the first real draw measures a container that by then has
    // a layout.
    const { map } = freshMap();
    expect(map._size).toBeUndefined();
    map.remove();
  });

  it('memoises a zero size once getSize() is called on an unlaid-out container', () => {
    const { map } = freshMap();
    const first = map.getSize();
    expect(first.x).toBe(0);
    expect(first.y).toBe(0);
    // Cached and marked clean — every later getSize() now returns this stale 0×0.
    expect(map._sizeChanged).toBe(false);
    map.remove();
  });

  it('cannot clear that cache with invalidateSize() before the map has a view', () => {
    // invalidateSize() returns early while `_loaded` is falsy, so it is exactly
    // no help in the one situation where the cache is poisoned. Gating the draw
    // on map.getSize() would therefore wedge the map shut forever.
    const { map } = freshMap();
    map.getSize();
    map.invalidateSize({ animate: false });
    expect(map._sizeChanged).toBe(false);
    map.remove();
  });

  it('reports the truth from the element itself, which is what GeoPreview gates on', () => {
    const { el, map } = freshMap();
    expect(el.clientWidth || el.clientHeight).toBeFalsy();
    map.remove();
  });
});
