/**
 * KaTeX and its stylesheet, imported only through `loadKatex` in
 * `markdown-math.tsx` so neither ships in the startup bundle.
 */
import katex from 'katex'
import 'katex/dist/katex.min.css'

export default katex
