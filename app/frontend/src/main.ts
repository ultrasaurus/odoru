import './style.css'
import { mount as mountReader } from './reader-author'
import { mount as mountEdit } from './edit'

let cleanup: (() => void) | null = null

function showReader() { cleanup?.(); cleanup = mountReader(showEdit) }
function showEdit()   { cleanup?.(); cleanup = mountEdit(showReader) }

showReader()
