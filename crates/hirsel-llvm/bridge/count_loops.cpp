// Loop cloning follows LLVM 23.1.4 LoopVersioning.cpp:88-115; the exit
// PHI repair generalizes its addPHINodes to all exits (overflow included).
#include <bits/c++config.h>
// The hermetic sysroot combines GCC 13's headers with older glibc headers.
// Use libstdc++'s ordinary POSIX lock implementations when the optional
// glibc 2.30 clock-lock declarations are unavailable.
#if defined(__GLIBC__) && !__GLIBC_PREREQ(2, 30)
#undef _GLIBCXX_USE_PTHREAD_COND_CLOCKWAIT
#undef _GLIBCXX_USE_PTHREAD_MUTEX_CLOCKLOCK
#undef _GLIBCXX_USE_PTHREAD_RWLOCK_CLOCKLOCK
#endif
#include <optional>
#include <llvm/Analysis/AssumptionCache.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ScalarEvolution.h>
#include <llvm/Analysis/ScalarEvolutionExpressions.h>
#include <llvm/Analysis/TargetLibraryInfo.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Passes/OptimizationLevel.h>
#include <llvm/Transforms/InstCombine/InstCombine.h>
#include <llvm/Transforms/Scalar/SimplifyCFG.h>
#include <llvm/Transforms/Utils/LCSSA.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>
#include <llvm/Transforms/Utils/Cloning.h>
#include <llvm/Transforms/Utils/LoopUtils.h>
#include <llvm/Transforms/Utils/LoopSimplify.h>
#include <llvm/Transforms/Utils/ScalarEvolutionExpander.h>
using namespace llvm;

// Each update is exactly the entry accumulator or its single +1, through
// PHI/select joins. Anything else, including another arithmetic update, refuses.
static bool countUpdate(Value *V, PHINode *Count, Value *Sum,
                        SmallPtrSetImpl<Value *> &Visiting) {
  if (V == Count || V == Sum) return true;
  if (!Visiting.insert(V).second) return false;
  bool OK = false;
  if (auto *P = dyn_cast<PHINode>(V)) {
    OK = true;
    for (Value *I : P->incoming_values())
      if (!countUpdate(I, Count, Sum, Visiting)) { OK = false; break; }
  } else if (auto *S = dyn_cast<SelectInst>(V)) {
    OK = countUpdate(S->getTrueValue(), Count, Sum, Visiting) &&
         countUpdate(S->getFalseValue(), Count, Sum, Visiting);
  }
  Visiting.erase(V);
  return OK;
}

static bool versionCount(Loop &L, LoopInfo &LI, DominatorTree &DT,
                         ScalarEvolution &SE) {
  BasicBlock *PH = L.getLoopPreheader(), *Latch = L.getLoopLatch();
  if (!PH || !Latch || !L.isInnermost() || L.getNumBlocks() > 24) return false;
  auto *End = dyn_cast<CondBrInst>(Latch->getTerminator());
  if (!End ||
      (L.contains(End->getSuccessor(0)) && L.contains(End->getSuccessor(1))))
    return false;
  // An iteration must be acyclic once the header backedges are removed:
  // this excludes irreducible cycles that could execute the +1 twice.
  DenseMap<BasicBlock *, unsigned> Degree;
  SmallVector<BasicBlock *, 16> Ready;
  for (BasicBlock *B : L.blocks()) Degree[B] = 0;
  for (BasicBlock *B : L.blocks())
    for (BasicBlock *S : successors(B))
      if (L.contains(S) && S != L.getHeader()) ++Degree[S];
  for (auto [B, D] : Degree) if (!D) Ready.push_back(B);
  unsigned Visited = 0;
  while (!Ready.empty()) {
    BasicBlock *B = Ready.pop_back_val(); ++Visited;
    for (BasicBlock *S : successors(B))
      if (L.contains(S) && S != L.getHeader() && !--Degree[S]) Ready.push_back(S);
  }
  if (Visited != L.getNumBlocks()) return false;

  IntrinsicInst *Add = nullptr;
  for (BasicBlock *B : L.blocks()) for (Instruction &I : *B) {
    if (auto *Call = dyn_cast<CallBase>(&I)) {
      auto *II = dyn_cast<IntrinsicInst>(Call);
      if (!II || II->getIntrinsicID() != Intrinsic::uadd_with_overflow || Add)
        return false;
      Add = II;
    } else if (I.mayWriteToMemory()) return false;
  }
  if (!Add) return false;
  auto *Count = dyn_cast<PHINode>(Add->getArgOperand(0));
  auto *One = dyn_cast<ConstantInt>(Add->getArgOperand(1));
  if (!Count || Count->getParent() != L.getHeader() ||
      !Count->getType()->isIntegerTy(64) || !One || !One->isOne() ||
      Count->getNumIncomingValues() != 2) return false;
  Value *Sum = nullptr;
  for (User *U : Add->users()) {
    auto *E = dyn_cast<ExtractValueInst>(U);
    if (!E || E->getNumIndices() != 1 || *E->idx_begin() > 1) return false;
    if (*E->idx_begin() == 0) {
      if (Sum) return false;
      Sum = E;
    }
  }
  if (!Sum) return false;
  Value *Initial = Count->getIncomingValueForBlock(PH);
  Value *Next = Count->getIncomingValueForBlock(Latch);
  SmallPtrSet<Value *, 8> Visiting;
  if (!Initial || !Next || Next == Sum || Next == Count ||
      !countUpdate(Next, Count, Sum, Visiting))
    return false;

  const SCEV *EC = SE.getExitCount(&L, Latch, ScalarEvolution::Exact);
  if (isa<SCEVCouldNotCompute>(EC) || !SE.isLoopInvariant(EC, &L) ||
      !EC->getType()->isIntegerTy() || EC->getType()->getIntegerBitWidth() > 64)
    return false;
  SCEVExpander Exp(SE, "count.bound");
  if (!Exp.isSafeToExpandAt(EC, PH->getTerminator())) return false;
  Value *Backedges = Exp.expandCodeFor(EC, EC->getType(), PH->getTerminator());
  IRBuilder<> Builder(PH->getTerminator());
  // 65 bits represent initial + exit-count + 1 without wrapping, including
  // the 2^64-iteration case. On the fast path, induction proves
  // count_k <= initial+k <= MAX, even when +1 is conditional.
  Type *Wide = Builder.getIntNTy(65);
  Value *Limit = Builder.CreateAdd(Builder.CreateZExt(Backedges, Wide),
                                  ConstantInt::get(Wide, 1));
  Value *Total = Builder.CreateAdd(Builder.CreateZExt(Initial, Wide), Limit);
  Value *Safe = Builder.CreateICmpULE(Total, ConstantInt::get(Wide, UINT64_MAX),
                                     "count.headroom");
  formLCSSA(L, DT, &LI, &SE);
  BasicBlock *Check = PH;
  PH = SplitBlock(Check, Check->getTerminator(), &DT, &LI, nullptr, "count.fast.ph");
  ValueToValueMapTy Map;
  SmallVector<BasicBlock *, 16> Cloned;
  Loop *Slow = cloneLoopWithPreheader(PH, Check, &L, Map, ".count.checked", &LI,
                                      &DT, Cloned);
  remapInstructionsInBlocks(Cloned, Map);
  // LCSSA already funnels all outside uses through exit PHIs. Clone each
  // incoming edge, preserving the precise partially updated state on failure.
  SmallVector<BasicBlock *, 8> Exits;
  L.getUniqueExitBlocks(Exits);
  for (BasicBlock *Exit : Exits) for (PHINode &P : Exit->phis()) {
    unsigned N = P.getNumIncomingValues();
    for (unsigned I = 0; I < N; ++I) {
      BasicBlock *From = P.getIncomingBlock(I);
      if (!L.contains(From)) continue;
      Value *V = P.getIncomingValue(I);
      P.addIncoming(Map.count(V) ? static_cast<Value *>(Map[V]) : V,
                    cast<BasicBlock>(Map[From]));
    }
  }
  Instruction *OldBranch = Check->getTerminator();
  CondBrInst::Create(Safe, PH, Slow->getLoopPreheader(), OldBranch->getIterator());
  OldBranch->eraseFromParent();
  IRBuilder<> Fast(Add);
  Value *Unchecked = Fast.CreateNUWAdd(Count, One, "count.proven");
  Value *Pair = PoisonValue::get(Add->getType());
  Pair = Fast.CreateInsertValue(Pair, Unchecked, {0});
  Pair = Fast.CreateInsertValue(Pair, Fast.getFalse(), {1});
  Add->replaceAllUsesWith(Pair);
  Add->eraseFromParent();
  return true;
}

// Run at VectorizerStart, before partial unrolling duplicates the increment.
class CountLoopPass : public PassInfoMixin<CountLoopPass> {
public:
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &AM) {
    // Most functions have no candidate, so avoid constructing analyses.
    bool Candidate = false;
    for (BasicBlock &B : F) for (Instruction &I : B) {
      auto *II = dyn_cast<IntrinsicInst>(&I);
      if (!II || II->getIntrinsicID() != Intrinsic::uadd_with_overflow) continue;
      auto *P = dyn_cast<PHINode>(II->getArgOperand(0));
      auto *One = dyn_cast<ConstantInt>(II->getArgOperand(1));
      Candidate |= P && P->getType()->isIntegerTy(64) && One && One->isOne();
    }
    if (!Candidate) return PreservedAnalyses::all();
    bool Versioned = false;
    {
      DominatorTree DT(F);
      LoopInfo LI(DT);
      AssumptionCache AC(F);
      TargetLibraryInfoImpl Impl(Triple(F.getParent()->getTargetTriple()));
      TargetLibraryInfo TLI(Impl);
      ScalarEvolution SE(F, TLI, AC, DT, LI);
      for (Loop *L : LI)
        simplifyLoop(L, &DT, &LI, &SE, &AC, nullptr, false);
      // At most one clone per function bounds code growth. The following
      // standard vectorizer consumes fresh analyses of both versions.
      for (Loop *L : LI.getLoopsInPreorder()) {
        if (versionCount(*L, LI, DT, SE)) {
          Versioned = true;
          break;
        }
      }
    }
    if (Versioned) {
      // Fold the proved-false overflow edge and if-convert the increment
      // before the standard vectorizer examines the new loop.
      AM.invalidate(F, PreservedAnalyses::none());
      FunctionPassManager Clean;
      Clean.addPass(InstCombinePass());
      Clean.addPass(SimplifyCFGPass());
      Clean.addPass(LoopSimplifyPass());
      Clean.addPass(LCSSAPass());
      Clean.run(F, AM);
    }
    return PreservedAnalyses::none();
  }
};

// Bounded loop cloning follows LLVM LoopVersioning.cpp:96-110. Unlike loop
// versioning, the clone returns all loop-carried SSA values to the outer header.
static bool chunkAhead(Loop &L, LoopInfo &LI, DominatorTree &DT) {
  constexpr uint64_t K = 32;
  BasicBlock *H = L.getHeader(), *PH = L.getLoopPreheader();
  BasicBlock *Latch = L.getLoopLatch();
  if (!PH || !Latch || !L.isInnermost() || L.getNumBlocks() > 64)
    return false;
  struct Point {
    CallInst *Handoff;
    BasicBlock *Cold, *Test, *Join;
    BinaryOperator *Dec;
  };
  SmallVector<Point, 8> Points;
  unsigned Instructions = 0;
  for (BasicBlock *B : L.blocks()) for (Instruction &I : *B) {
    if (++Instructions > 512) return false;
    if (auto *Call = dyn_cast<CallBase>(&I)) {
      if (auto *II = dyn_cast<IntrinsicInst>(Call)) {
        if (!II->mayReadOrWriteMemory() && !II->mayHaveSideEffects()) continue;
        return false;
      }
      auto *C = dyn_cast<CallInst>(Call);
      auto *F = C ? C->getCalledFunction() : nullptr;
      if (!F || F->getName() != "hirsel_rt_v2_ahead_handoff" ||
          C->arg_size() != 2 || !isa<ConstantInt>(C->getArgOperand(1)) ||
          !cast<ConstantInt>(C->getArgOperand(1))->isOne()) return false;
      BasicBlock *Cold = C->getParent(), *Test = Cold->getSinglePredecessor();
      BasicBlock *Join = Cold->getSingleSuccessor();
      auto *Check = Test ? dyn_cast<CondBrInst>(Test->getTerminator()) : nullptr;
      if (!Check || !Join || !DT.dominates(Test, Latch) ||
          !L.contains(Test) || !L.contains(Join)) return false;
      auto *Expired = dyn_cast<ICmpInst>(Check->getCondition());
      if (!Expired || Expired->getPredicate() != ICmpInst::ICMP_EQ ||
          Check->getSuccessor(0) != Cold || Check->getSuccessor(1) != Join)
        return false;
      auto *Zero = dyn_cast<ConstantInt>(Expired->getOperand(1));
      auto *Dec = dyn_cast<BinaryOperator>(Expired->getOperand(0));
      if (!Zero || !Zero->isZero() || !Dec || Dec->getOpcode() != Instruction::Add)
        return false;
      auto *MinusOne = dyn_cast<ConstantInt>(Dec->getOperand(1));
      if (!MinusOne || !MinusOne->isMinusOne()) return false;
      Points.push_back({C, Cold, Test, Join, Dec});
    } else if (auto *Load = dyn_cast<LoadInst>(&I)) {
      if (Load->isVolatile() || Load->isAtomic()) return false;
    } else if (auto *Store = dyn_cast<StoreInst>(&I)) {
      if (Store->isVolatile() || Store->isAtomic()) return false;
    } else if (I.mayReadOrWriteMemory() || I.mayHaveSideEffects()) {
      if (!I.isTerminator()) return false;
    }
  }
  if (Points.empty() || Points.size() > 8) return false;
  for (const Point &P : Points) for (Instruction &I : *P.Cold) {
    if (auto *Load = dyn_cast<LoadInst>(&I)) {
      if (Load->getPointerOperand() != P.Handoff->getArgOperand(0)) return false;
    } else if (auto *Store = dyn_cast<StoreInst>(&I)) {
      if (Store->getPointerOperand() != P.Handoff->getArgOperand(0)) return false;
    }
  }
  // Every path to the latch must consume this exact chain of physical points.
  // A cold refill is the only permitted alternative at each join. Partial
  // iterations keep their decrements on every source/failure exit.
  PHINode *Quota = nullptr;
  auto *Earliest = Points.front().Test;
  for (const Point &P : Points)
    if (DT.dominates(P.Test, Earliest)) Earliest = P.Test;
  for (const Point &P : Points) if (P.Test == Earliest)
    Quota = dyn_cast<PHINode>(P.Dec->getOperand(0));
  if (!Quota || Quota->getParent() != H || !Quota->getType()->isIntegerTy(64) ||
      Quota->getNumIncomingValues() != 2) return false;
  SmallPtrSet<Value *, 16> Visiting;
  auto Cost = [&](auto &&Self, Value *V) -> std::optional<unsigned> {
    if (V == Quota) return 0;
    if (!Visiting.insert(V).second) return std::nullopt;
    std::optional<unsigned> Result;
    for (const Point &P : Points) {
      if (V == P.Dec) {
        Result = Self(Self, P.Dec->getOperand(0));
        if (Result) ++*Result;
        break;
      }
    }
    if (auto *Phi = dyn_cast<PHINode>(V); !Result && Phi &&
        Phi->getParent() != H && L.contains(Phi)) {
      bool OK = true;
      for (unsigned I = 0; I != Phi->getNumIncomingValues(); ++I) {
        BasicBlock *From = Phi->getIncomingBlock(I);
        bool Cold = llvm::any_of(Points, [&](const Point &P) { return P.Cold == From; });
        if (Cold) continue;
        auto C = Self(Self, Phi->getIncomingValue(I));
        if (!C || (Result && *Result != *C)) { OK = false; break; }
        Result = C;
      }
      if (!OK) Result = std::nullopt;
    }
    Visiting.erase(V);
    return Result;
  };
  auto PerTrip = Cost(Cost, Quota->getIncomingValueForBlock(Latch));
  if (!PerTrip || *PerTrip != Points.size()) return false;
  // Removing header feedback must leave a DAG, including all handoff
  // diamonds. No hidden cycle may consume another point or write twice.
  DenseMap<BasicBlock *, unsigned> Degree;
  SmallVector<BasicBlock *, 16> Ready;
  for (BasicBlock *B : L.blocks()) Degree[B] = 0;
  for (BasicBlock *B : L.blocks()) for (BasicBlock *S : successors(B))
    if (L.contains(S) && S != H) ++Degree[S];
  for (auto [B, D] : Degree) if (!D) Ready.push_back(B);
  unsigned Visited = 0;
  while (!Ready.empty()) {
    BasicBlock *B = Ready.pop_back_val(); ++Visited;
    for (BasicBlock *S : successors(B))
      if (L.contains(S) && S != H && !--Degree[S]) Ready.push_back(S);
  }
  if (Visited != L.getNumBlocks()) return false;

  PHINode *Index = nullptr;
  Value *Limit = nullptr;
  BasicBlock *Exit = nullptr;
  CondBrInst *End = nullptr;
  SmallVector<BasicBlock *, 8> Exiting;
  L.getExitingBlocks(Exiting);
  for (BasicBlock *Candidate : Exiting) {
    auto *Branch = dyn_cast<CondBrInst>(Candidate->getTerminator());
    auto *Cmp = Branch ? dyn_cast<ICmpInst>(Branch->getCondition()) : nullptr;
    if (!Cmp || !DT.dominates(Candidate, Latch)) continue;
    for (PHINode &P : H->phis()) {
      if (&P == Quota || !P.getType()->isIntegerTy(64) ||
          P.getNumIncomingValues() != 2) continue;
      auto *Next = dyn_cast_or_null<BinaryOperator>(P.getIncomingValueForBlock(Latch));
      if (!Next || Next->getOpcode() != Instruction::Add || Next->getOperand(0) != &P)
        continue;
      auto *One = dyn_cast<ConstantInt>(Next->getOperand(1));
      if (!One || !One->isOne()) continue;
      for (unsigned Side = 0; Side != 2; ++Side) {
        Value *V = Cmp->getOperand(Side), *Bound = Cmp->getOperand(1 - Side);
        if ((V != &P && V != Next) || !L.isLoopInvariant(Bound) ||
            !DT.dominates(Bound, PH->getTerminator())) continue;
        auto Pred = Side ? Cmp->getSwappedPredicate() : Cmp->getPredicate();
        bool TrueInside = L.contains(Branch->getSuccessor(0));
        if ((Pred == ICmpInst::ICMP_EQ && !TrueInside) ||
            (Pred == ICmpInst::ICMP_NE && TrueInside) ||
            (Pred == ICmpInst::ICMP_ULT && TrueInside) ||
            (Pred == ICmpInst::ICMP_UGE && !TrueInside)) {
          Index = &P; Limit = Bound; Exit = Candidate; End = Branch;
        }
      }
    }
  }
  if (!Index) return false;
  for (PHINode &P : H->phis())
    if (P.getNumIncomingValues() != 2 || P.getBasicBlockIndex(PH) < 0 ||
        P.getBasicBlockIndex(Latch) < 0) return false;

  formLCSSA(L, DT, &LI, nullptr);
  Function *F = H->getParent();
  ValueToValueMapTy Map;
  SmallVector<BasicBlock *, 16> Cloned;
  for (BasicBlock *B : L.blocks())
    Cloned.push_back(CloneBasicBlock(B, Map, ".ahead.chunk", F));
  for (unsigned I = 0; I != Cloned.size(); ++I) Map[L.getBlocks()[I]] = Cloned[I];
  remapInstructionsInBlocks(Cloned, Map);
  SmallVector<BasicBlock *, 8> Exits;
  L.getUniqueExitBlocks(Exits);
  for (BasicBlock *Out : Exits) for (PHINode &P : Out->phis()) {
    unsigned N = P.getNumIncomingValues();
    for (unsigned I = 0; I != N; ++I) {
      BasicBlock *From = P.getIncomingBlock(I);
      if (!L.contains(From)) continue;
      Value *V = P.getIncomingValue(I);
      P.addIncoming(Map.count(V) ? static_cast<Value *>(Map[V]) : V,
                    cast<BasicBlock>(Map[From]));
    }
  }
  auto *FastH = cast<BasicBlock>(Map[H]);
  auto *FastLatch = cast<BasicBlock>(Map[Latch]);
  auto *FastNext = BasicBlock::Create(F->getContext(), "ahead.chunk.next", F);
  auto *Outer = BasicBlock::Create(F->getContext(), "ahead.boundary", F, H);
  SmallVector<PHINode *, 8> Phis;
  for (PHINode &P : H->phis()) Phis.push_back(&P);
  for (PHINode *P : Phis) {
    auto *FastP = cast<PHINode>(Map[P]);
    int Entry = FastP->getBasicBlockIndex(PH);
    FastP->setIncomingBlock(Entry, Outer);
    FastP->setIncomingValue(Entry, P);
    FastP->setIncomingBlock(FastP->getBasicBlockIndex(FastLatch), FastNext);
    Value *Next = P->getIncomingValueForBlock(Latch);
    P->addIncoming(Map.count(Next) ? static_cast<Value *>(Map[Next]) : Next, FastNext);
    P->moveBefore(*Outer, Outer->end());
  }
  PH->getTerminator()->replaceSuccessorWith(H, Outer);
  Latch->getTerminator()->replaceSuccessorWith(H, Outer);
  IRBuilder<> B(Outer);
  Value *Spent = B.CreateSub(Quota, B.getInt64(K * *PerTrip), "ahead.chunk.remaining");
  Quota->setIncomingValue(Quota->getBasicBlockIndex(FastNext), Spent);
  // The original checkpoint may diverge before the source bound is read.
  // As in LoopUnrollRuntime.cpp:847-860, speculative guards must not turn
  // a not-yet-observed poison into immediate branch UB.
  Value *GuardQuota = B.CreateFreeze(Quota, "ahead.quota");
  Value *GuardIndex = B.CreateFreeze(Index, "ahead.index");
  Value *GuardLimit = B.CreateFreeze(Limit, "ahead.limit");
  Value *Room = B.CreateICmpUGT(GuardQuota, B.getInt64(K * *PerTrip));
  Value *Remaining = B.CreateSub(GuardLimit, GuardIndex);
  Value *Bounded = B.CreateAnd(B.CreateICmpULT(GuardIndex, GuardLimit),
                              B.CreateICmpUGT(Remaining, B.getInt64(K)));
  B.CreateCondBr(B.CreateAnd(Room, Bounded, "ahead.chunk.room"), FastH, H);
  auto Force = [&](BasicBlock *Original, BasicBlock *Successor) {
    auto *BB = cast<BasicBlock>(Map[Original]);
    auto *Old = BB->getTerminator();
    auto *Next = cast<BasicBlock>(Map[Successor]);
    for (BasicBlock *S : successors(BB))
      if (S != Next) S->removePredecessor(BB);
    UncondBrInst::Create(Next, Old->getIterator());
    Old->eraseFromParent();
  };
  for (const Point &P : Points) Force(P.Test, P.Join);
  Force(Exit, L.contains(End->getSuccessor(0)) ? End->getSuccessor(0) : End->getSuccessor(1));
  B.SetInsertPoint(FastH, FastH->begin());
  auto *Trips = B.CreatePHI(B.getInt64Ty(), 2, "ahead.chunk.trip");
  Trips->addIncoming(B.getInt64(0), Outer);
  FastLatch->getTerminator()->replaceSuccessorWith(FastH, FastNext);
  B.SetInsertPoint(FastNext);
  Value *NextTrip = B.CreateAdd(Trips, B.getInt64(1));
  Trips->addIncoming(NextTrip, FastNext);
  auto *Backedge = B.CreateCondBr(B.CreateICmpULT(NextTrip, B.getInt64(K)), FastH, Outer);
  auto &C = F->getContext();
  Metadata *Unroll[] = {MDString::get(C, "llvm.loop.unroll.count"),
                       ConstantAsMetadata::get(B.getInt32(4))};
  Metadata *LoopID[] = {nullptr, MDNode::get(C, Unroll)};
  auto *ID = MDNode::getDistinct(C, LoopID);
  ID->replaceOperandWith(0, ID);
  Backedge->setMetadata(LLVMContext::MD_loop, ID);
  EliminateUnreachableBlocks(*F);
  return true;
}

class AheadChunkPass : public PassInfoMixin<AheadChunkPass> {
public:
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &AM) {
    if (!F.getName().ends_with("_ahead")) return PreservedAnalyses::all();
    bool Changed = false;
    SmallPtrSet<BasicBlock *, 16> Tried;
    for (unsigned N = 0; N != 8; ++N) {
      bool Progress = false;
      DominatorTree DT(F);
      LoopInfo LI(DT);
      for (Loop *L : LI.getLoopsInPreorder()) {
        BasicBlock *H = L->getHeader();
        if (H->getName().contains(".ahead.chunk") || !Tried.insert(H).second) continue;
        if (chunkAhead(*L, LI, DT)) { Progress = true; Changed = true; break; }
      }
      if (!Progress) break;
    }
    if (!Changed) return PreservedAnalyses::all();
    AM.invalidate(F, PreservedAnalyses::none());
    FunctionPassManager Clean;
    Clean.addPass(InstCombinePass());
    Clean.addPass(SimplifyCFGPass());
    Clean.addPass(LoopSimplifyPass());
    Clean.addPass(LCSSAPass());
    Clean.run(F, AM);
    return PreservedAnalyses::none();
  }
};

extern "C" void hirsel_llvm_count_pipeline(void *Raw, void *Machine,
                                          unsigned Level) noexcept {
  auto &M = *static_cast<Module *>(Raw);
  PassBuilder PB(static_cast<TargetMachine *>(Machine));
  LoopAnalysisManager LAM;
  FunctionAnalysisManager FAM;
  CGSCCAnalysisManager CGAM;
  ModuleAnalysisManager MAM;
  PB.registerModuleAnalyses(MAM);
  PB.registerCGSCCAnalyses(CGAM);
  PB.registerFunctionAnalyses(FAM);
  PB.registerLoopAnalyses(LAM);
  PB.crossRegisterProxies(LAM, FAM, CGAM, MAM);
  PB.registerVectorizerStartEPCallback(
      [](FunctionPassManager &PM, OptimizationLevel) {
        PM.addPass(CountLoopPass());
        PM.addPass(AheadChunkPass());
      });
  auto PM = PB.buildPerModuleDefaultPipeline(
      Level == 3 ? OptimizationLevel::O3 : OptimizationLevel::O2);
  PM.run(M, MAM);
}
