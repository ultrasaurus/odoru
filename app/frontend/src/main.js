import './style.css';
import { mount as mountReader } from './reader-author';
import { mount as mountEdit } from './edit';
let cleanup = null;
const LAST_VIEW_KEY = 'odoru:lastView';
function showReader() {
    cleanup?.();
    localStorage.setItem(LAST_VIEW_KEY, 'reader');
    cleanup = mountReader(showEdit);
}
function showEdit() {
    cleanup?.();
    localStorage.setItem(LAST_VIEW_KEY, 'edit');
    cleanup = mountEdit(showReader);
}
if (localStorage.getItem(LAST_VIEW_KEY) === 'reader')
    showReader();
else
    showEdit();
