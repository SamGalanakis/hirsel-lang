// Copyright 2010 The Go Authors. All rights reserved.
// Use of this source code is governed by the BSD license in GO-LICENSE.txt.
// Reproduction helper for the pinned token_test.go vectors. No build products
// are committed. Run generate.py /path/to/x-net /path/to/go.
package main

import (
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"golang.org/x/net/html"
	"io"
	"os"
	"strconv"
	"strings"
)

type Case struct {
	Name        string
	Source      string
	CDATA       bool
	SuppressRaw bool
	Tokens      []Token
}
type Token struct {
	Kind  string
	Start int
	End   int
	Data  string
	Attr  []html.Attribute
}

func eval(e ast.Expr, constants map[string]string) string {
	switch e := e.(type) {
	case *ast.BasicLit:
		s, err := strconv.Unquote(e.Value)
		if err != nil {
			panic(err)
		}
		return s
	case *ast.Ident:
		return constants[e.Name]
	case *ast.BinaryExpr:
		return eval(e.X, constants) + eval(e.Y, constants)
	}
	panic("non-string vector")
}

func main() {
	cases := []Case{}
	json.NewDecoder(os.Stdin).Decode(&cases)
	file, err := parser.ParseFile(token.NewFileSet(), os.Args[1], nil, 0)
	if err != nil {
		panic(err)
	}
	constants := map[string]string{}
	ast.Inspect(file, func(n ast.Node) bool {
		v, ok := n.(*ast.ValueSpec)
		if !ok {
			return true
		}
		if len(v.Names) == 1 && v.Names[0].Name == "issue58246" {
			constants["issue58246"] = eval(v.Values[0], constants)
		}
		if len(v.Names) != 1 || v.Names[0].Name != "tokenTests" {
			return true
		}
		for _, row := range v.Values[0].(*ast.CompositeLit).Elts {
			cols := row.(*ast.CompositeLit).Elts
			cases = append(cases, Case{Name: "upstream/" + eval(cols[0], constants), Source: eval(cols[1], constants), CDATA: true})
		}
		return false
	})
	for i := range cases {
		c := &cases[i]
		z := html.NewTokenizer(strings.NewReader(c.Source))
		z.AllowCDATA(c.CDATA)
		offset := 0
		for {
			if c.SuppressRaw {
				z.NextIsNotRawText()
			}
			kind := z.Next()
			end := offset + len(z.Raw())
			if kind == html.ErrorToken {
				if z.Err() != io.EOF {
					panic(z.Err())
				}
				break
			}
			t := z.Token()
			c.Tokens = append(c.Tokens, Token{kind.String(), offset, end, t.Data, t.Attr})
			offset = end
		}
	}
	if err := json.NewEncoder(os.Stdout).Encode(cases); err != nil {
		panic(err)
	}
}
