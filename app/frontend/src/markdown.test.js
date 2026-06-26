import { describe, expect, it } from 'vitest';
import { stripSilent } from './markdown';
describe('stripSilent', () => {
    it('removes a fully-silent inline line entirely', () => {
        const input = 'Before\n[Doug Engelbart]<!--silent-->\nAfter';
        expect(stripSilent(input)).toBe('Before\nAfter');
    });
    it('removes a fully-silent heading entirely', () => {
        const input = '# [Navigation]<!--silent-->\n\nBody text';
        expect(stripSilent(input)).toBe('\nBody text');
    });
    it('leaves normal text and blank-line paragraph spacing untouched', () => {
        const input = 'First paragraph.\n\nSecond paragraph.';
        expect(stripSilent(input)).toBe(input);
    });
    it('strips a mid-line silent span but keeps the rest of the line', () => {
        const input = 'Some text [Doug Engelbart]<!--silent--> continues here.';
        expect(stripSilent(input)).toBe('Some text  continues here.');
    });
});
