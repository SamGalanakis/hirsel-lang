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
      });
  auto PM = PB.buildPerModuleDefaultPipeline(
      Level == 3 ? OptimizationLevel::O3 : OptimizationLevel::O2);
  PM.run(M, MAM);
}
