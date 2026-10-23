// Oracle generator; run in a module replacing github.com/restic/chunker with
// pinned commit 2e8f53f1dad5dbdcf8aca1dbf1bee7d1fc9d8ce8. BSD license with app.
package main
import("bytes";"crypto/sha256";"encoding/hex";"encoding/json";"fmt";"io";"os";"github.com/restic/chunker")
type Part struct { Offset uint `json:"offset"`; Length uint `json:"length"`; ID string `json:"id"` }
type Vector struct { Name string `json:"name"`; Data string `json:"hex"`; Parts []Part `json:"parts"` }
func main(){
 vectors:=[]Vector{}
 random:=make([]byte,8192); state:=uint32(23);for i:=range random{state=state*1664525+1013904223;random[i]=byte(state>>24)}
 for _,test:=range []struct{name string;data []byte}{{"empty",nil},{"tiny",[]byte{0,255,1}},{"zero255",make([]byte,255)},{"zero256",make([]byte,256)},{"zero257",make([]byte,257)},{"zero512",make([]byte,512)},{"zero4096",make([]byte,4096)},{"forced4096",bytes.Repeat([]byte{1},4096)},{"seed8192",random},{"shift8192",append([]byte("shift"),random...)}}{
  c:=chunker.New(bytes.NewReader(test.data),0x3DA3358B4DC173,chunker.WithBoundaries(256,4096),chunker.WithAverageBits(10));parts:=[]Part{}
  var reconstructed []byte
  for {p,e:=c.Next(nil);if e==io.EOF{break};if e!=nil{panic(e)};h:=sha256.Sum256(p.Data);parts=append(parts,Part{p.Start,p.Length,fmt.Sprintf("%x",h)});reconstructed=append(reconstructed,p.Data...)}
  if !bytes.Equal(reconstructed,test.data){panic("reconstruction")}
  c.Reset(bytes.NewReader(test.data),0x3DA3358B4DC173,chunker.WithBoundaries(256,4096),chunker.WithAverageBits(10));for _,want:=range parts{p,e:=c.Next(nil);if e!=nil||p.Start!=want.Offset||p.Length!=want.Length{panic("reset")}}
  // Independent fragmentation invariance on the upstream BaseChunker.
  for _,stride:=range []int{1,7,64,257,4096}{b:=chunker.NewBase(0x3DA3358B4DC173,chunker.WithBaseBoundaries(256,4096),chunker.WithBaseAverageBits(10));pos:=0;idx:=0;for pos<len(test.data){end:=pos+stride;if end>len(test.data){end=len(test.data)};n:=b.NextSplitPoint(test.data[pos:end]);if n<0{pos=end}else{pos+=n;if idx>=len(parts)||uint(pos)!=parts[idx].Offset+parts[idx].Length{panic("fragment")};idx++}};if idx<len(parts)&&uint(pos)!=parts[idx].Offset+parts[idx].Length{panic("tail")}}
  vectors=append(vectors,Vector{test.name,hex.EncodeToString(test.data),parts})
 }
 if e:=json.NewEncoder(os.Stdout).Encode(vectors);e!=nil{panic(e)}
}
