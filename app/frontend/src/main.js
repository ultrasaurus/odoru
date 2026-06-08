import './style.css';
import { mount as mountReader } from './reader-author';
import { mount as mountEdit } from './edit';
let cleanup = null;
function showReader() { cleanup?.(); cleanup = mountReader(showEdit); }
function showEdit() { cleanup?.(); cleanup = mountEdit(showReader); }
showReader();
